// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #463 V1 (register §1.378; contracts/package-variables.md): a package's `variables`
//! read by the one reader of the contract crate, with the goal declaration's value-type code: the
//! four kinds read with their members; each refusal at its pointer with its word; a name a
//! parameter has is refused. The root check: byte-identical declarations of one name share one
//! variable, a differing one, a name a `goals.json` entry uses, a 97th name and an `integer` read
//! as a cap above 1000 do not apply, each reported at its pointer; the provenance listing names
//! each variable that applies by its default's origin. A placement is held to the variables the
//! root declares. Fix round 1: a cap read from a negative `min` or from a variable of another type
//! is reported. Workflow #455 G-SLx (register §1.501): a package that provides an irreversible
//! capability declares variables as any other (V1 refused them, `variables_on_irreversible`), and
//! a variable of a package that counts as irreversible by the whole root applies (V1 fix round 1
//! kept it from applying, `variable_irreversible_by_root`). Every package, name, option and text
//! is made up.

use super::*;
use crate::standard_pack_goals::{
    DeclaredGoalKind, GoalReferences, StandardPackGoals, check_standard_pack_goal_references,
    parse_standard_pack_goals,
};
use serde_json::json;

/// The four kinds as the design writes them, with made-up names: a choice whose default the
/// model wrote, a number on a step, an order of three options, a switch.
fn declared() -> Value {
    json!([
        {"name": "pick", "type": "choice", "title": {"en": "Item", "zh-Hans": "要挑的那一项"},
         "options": [{"value": "item_a", "title": {"en": "Item A"}},
                     {"value": "item_b", "title": {"en": "Item B"}}],
         "default": "item_b", "default_origin": "model"},
        {"name": "floor", "type": "integer", "title": {"en": "Keep at least"},
         "min": 0, "max": 200, "step": 10, "default": 0},
        {"name": "order", "type": "ordered", "title": {"en": "Order"},
         "options": [{"value": "item_a", "title": {"en": "Item A"}},
                     {"value": "item_b", "title": {"en": "Item B"}},
                     {"value": "item_c", "title": {"en": "Item C"}}],
         "min_count": 0, "max_count": 8, "default": ["item_b", "item_a"]},
        {"name": "extra", "type": "switch", "title": {"en": "Also the extra"}, "default": false}
    ])
}

/// [`declared`] changed by `change`.
fn with(change: impl Fn(&mut Value)) -> Value {
    let mut value = declared();
    change(&mut value);
    value
}

/// One capability with the parameter `param_p`, irreversible when `irreversible`.
fn provides(irreversible: bool) -> Vec<ProvidedCapability> {
    serde_json::from_value(json!([
        {"capability": "cap_x", "irreversible": irreversible, "parameters": [
            {"name": "param_p", "options": ["opt_1", "opt_2"], "default": "opt_1"}]}
    ]))
    .expect("the capabilities")
}

fn text(pairs: &[(&str, &str)]) -> GoalText {
    pairs
        .iter()
        .map(|(locale, written)| ((*locale).to_owned(), (*written).to_owned()))
        .collect()
}

fn option(value: &str, title: &str) -> GoalOption {
    GoalOption {
        value: value.to_owned(),
        title: text(&[("en", title)]),
    }
}

fn owned(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn v1_each_kind_reads_with_its_members() {
    let variables = parse_package_variables(&declared(), &provides(false))
        .unwrap_or_else(|violation| panic!("the four kinds read: {violation:?}"));
    assert_eq!(
        variables
            .iter()
            .map(|variable| (
                variable.name.as_str(),
                variable.variable_type(),
                variable.block,
                variable.default_origin
            ))
            .collect::<Vec<_>>(),
        [
            (
                "pick",
                VariableType::Choice,
                GoalBlock::Choice,
                DefaultOrigin::Model
            ),
            (
                "floor",
                VariableType::Integer,
                GoalBlock::Number,
                DefaultOrigin::Pack
            ),
            (
                "order",
                VariableType::Ordered,
                GoalBlock::OrderedList,
                DefaultOrigin::Pack
            ),
            (
                "extra",
                VariableType::Switch,
                GoalBlock::Switch,
                DefaultOrigin::Pack
            ),
        ]
    );
    assert_eq!(
        variables
            .iter()
            .map(|variable| &variable.kind)
            .collect::<Vec<_>>(),
        [
            &VariableKind::Choice {
                options: vec![option("item_a", "Item A"), option("item_b", "Item B")],
                default: Some("item_b".to_owned()),
            },
            &VariableKind::Integer {
                min: 0,
                max: 200,
                step: 10,
                unit: None,
                default: Some(0),
            },
            &VariableKind::Ordered {
                options: vec![
                    option("item_a", "Item A"),
                    option("item_b", "Item B"),
                    option("item_c", "Item C"),
                ],
                min_count: 0,
                max_count: 8,
                default: Some(owned(&["item_b", "item_a"])),
            },
            &VariableKind::Switch {
                default: Some(false)
            },
        ]
    );
    assert_eq!(
        variables[0].title,
        text(&[("en", "Item"), ("zh-Hans", "要挑的那一项")])
    );
    assert_eq!(variables[0].help, None);
    assert_eq!(variables[1].declaration, declared()[1]);
    assert_eq!(variables[1].fact_key(), "session.dispatch.parameter.floor");

    // The base block written, a time unit, a help text and sixteen entries read too, and so
    // does a package that provides nothing.
    let mut sixteen = with(|value| {
        value[1]["block"] = json!("number");
        value[1]["unit"] = json!("minute");
        value[3]["help"] = json!({"en": "On or off."});
    });
    let entries = sixteen.as_array_mut().expect("the list");
    for index in 4..16 {
        entries.push(json!({"name": format!("extra_{index}"), "type": "switch",
                            "title": {"en": "Extra"}, "default": true}));
    }
    let read = parse_package_variables(&sixteen, &[])
        .unwrap_or_else(|violation| panic!("sixteen entries read: {violation:?}"));
    assert_eq!(read.len(), MAX_PACKAGE_VARIABLES);
    assert_eq!(
        read[1].kind,
        VariableKind::Integer {
            min: 0,
            max: 200,
            step: 10,
            unit: Some(GoalUnit::Time(GoalTimeUnit::Minute)),
            default: Some(0),
        }
    );
    assert_eq!(read[3].help, Some(text(&[("en", "On or off.")])));
}

#[test]
fn v1_each_refusal_is_at_its_pointer() {
    use VariablesProblem::{VariableNameTaken, VariablesInvalid};
    let seventeen = Value::Array(
        (0..17)
            .map(|index| {
                json!({"name": format!("extra_{index}"), "type": "switch",
                       "title": {"en": "Extra"}, "default": true})
            })
            .collect(),
    );
    let cases = vec![
        (
            "no list",
            json!({"name": "pick"}),
            Vec::new(),
            VariablesInvalid,
            "/variables",
        ),
        (
            "no entry",
            json!([]),
            Vec::new(),
            VariablesInvalid,
            "/variables",
        ),
        (
            "seventeen entries",
            seventeen,
            Vec::new(),
            VariablesInvalid,
            "/variables",
        ),
        (
            "an entry that is no object",
            with(|value| value[0] = json!("pick")),
            Vec::new(),
            VariablesInvalid,
            "/variables/0",
        ),
        (
            "an unknown member",
            with(|value| value[0]["extra"] = json!(true)),
            Vec::new(),
            VariablesInvalid,
            "/variables/0/extra",
        ),
        (
            "a goal entry's governs",
            with(|value| value[1]["governs"] = json!({"packages": ["neutral.collect"]})),
            Vec::new(),
            VariablesInvalid,
            "/variables/1/governs",
        ),
        (
            "a table, which a later slice brings",
            with(|value| value[2]["from_table"] = json!({"table": "neutral_members"})),
            Vec::new(),
            VariablesInvalid,
            "/variables/2/from_table",
        ),
        (
            "a missing title",
            with(|value| {
                value[3].as_object_mut().expect("an entry").remove("title");
            }),
            Vec::new(),
            VariablesInvalid,
            "/variables/3/title",
        ),
        (
            "a name outside the token form",
            with(|value| value[0]["name"] = json!("Pick")),
            Vec::new(),
            VariablesInvalid,
            "/variables/0/name",
        ),
        (
            "a name that begins with a digit",
            with(|value| value[0]["name"] = json!("1pick")),
            Vec::new(),
            VariablesInvalid,
            "/variables/0/name",
        ),
        (
            "a name of 65 bytes",
            with(|value| value[0]["name"] = json!("a".repeat(65))),
            Vec::new(),
            VariablesInvalid,
            "/variables/0/name",
        ),
        (
            "an unknown type",
            with(|value| value[0]["type"] = json!("slider")),
            Vec::new(),
            VariablesInvalid,
            "/variables/0/type",
        ),
        (
            "a goal entry's type",
            with(|value| value[0]["type"] = json!("tier")),
            Vec::new(),
            VariablesInvalid,
            "/variables/0/type",
        ),
        (
            "a block that is not the type's",
            with(|value| value[1]["block"] = json!("choice")),
            Vec::new(),
            VariablesInvalid,
            "/variables/1/block",
        ),
        (
            "a choice of one option",
            with(|value| {
                value[0]["options"] = json!([{"value": "item_b", "title": {"en": "Item B"}}]);
            }),
            Vec::new(),
            VariablesInvalid,
            "/variables/0/options",
        ),
        (
            "an option value outside the token form",
            with(|value| value[0]["options"][0]["value"] = json!("item-a")),
            Vec::new(),
            VariablesInvalid,
            "/variables/0/options/0/value",
        ),
        (
            "an option written twice",
            with(|value| value[0]["options"][1]["value"] = json!("item_a")),
            Vec::new(),
            VariablesInvalid,
            "/variables/0/options/1/value",
        ),
        (
            "a choice's default that is no option",
            with(|value| value[0]["default"] = json!("item_c")),
            Vec::new(),
            VariablesInvalid,
            "/variables/0/default",
        ),
        (
            "an integer's min above its max",
            with(|value| value[1]["min"] = json!(300)),
            Vec::new(),
            VariablesInvalid,
            "/variables/1/min",
        ),
        (
            "a step below 1",
            with(|value| value[1]["step"] = json!(0)),
            Vec::new(),
            VariablesInvalid,
            "/variables/1/step",
        ),
        (
            "a default off the step",
            with(|value| value[1]["default"] = json!(5)),
            Vec::new(),
            VariablesInvalid,
            "/variables/1/default",
        ),
        (
            "a unit that is no time word",
            with(|value| value[1]["unit"] = json!("week")),
            Vec::new(),
            VariablesInvalid,
            "/variables/1/unit",
        ),
        (
            "a max_count above 32",
            with(|value| value[2]["max_count"] = json!(33)),
            Vec::new(),
            VariablesInvalid,
            "/variables/2/max_count",
        ),
        (
            "a min_count above the options",
            with(|value| value[2]["min_count"] = json!(4)),
            Vec::new(),
            VariablesInvalid,
            "/variables/2/min_count",
        ),
        (
            "an ordered default that names no option",
            with(|value| value[2]["default"] = json!(["item_b", "item_d"])),
            Vec::new(),
            VariablesInvalid,
            "/variables/2/default/1",
        ),
        (
            "a switch's default that is no boolean",
            with(|value| value[3]["default"] = json!("no")),
            Vec::new(),
            VariablesInvalid,
            "/variables/3/default",
        ),
        (
            "an origin other than the model",
            with(|value| value[0]["default_origin"] = json!("pack")),
            Vec::new(),
            VariablesInvalid,
            "/variables/0/default_origin",
        ),
        (
            "a name an earlier variable has",
            with(|value| value[3]["name"] = json!("pick")),
            Vec::new(),
            VariableNameTaken,
            "/variables/3/name",
        ),
        (
            "a name a parameter of provides has",
            with(|value| value[1]["name"] = json!("param_p")),
            provides(false),
            VariableNameTaken,
            "/variables/1/name",
        ),
        // Workflow #455 G-SLx (register §1.501): a parameter of a capability marked irreversible
        // takes its name as any parameter does; the mark itself refuses nothing.
        (
            "a name a parameter of an irreversible capability has",
            with(|value| value[1]["name"] = json!("param_p")),
            provides(true),
            VariableNameTaken,
            "/variables/1/name",
        ),
    ];
    for (case, variables, provides, problem, pointer) in cases {
        assert_eq!(
            parse_package_variables(&variables, &provides).map(|_| ()),
            Err(VariablesViolation {
                problem,
                pointer: pointer.to_owned(),
            }),
            "{case}"
        );
    }
    // Workflow #455 G-SLx: a package that provides an irreversible capability declares the four
    // kinds as a package that provides a reversible one does (until 0.12.77 refused
    // `variables_on_irreversible` at `/variables`).
    assert_eq!(
        parse_package_variables(&declared(), &provides(true)),
        parse_package_variables(&declared(), &provides(false))
    );
    assert_eq!(
        parse_package_variables(&declared(), &provides(true)).map(|read| read.len()),
        Ok(4)
    );
    // The words, as kernel admission's `reason` and the source parser's message write them.
    assert_eq!(
        [VariablesInvalid, VariableNameTaken].map(VariablesProblem::as_str),
        ["variables_invalid", "variable_name_taken"]
    );
}

/// The package `package_id` of a root, its `variables` read with no `provides`, reading its caps
/// from `cap_fact_keys`.
fn package(package_id: &str, variables: Value, cap_fact_keys: &[&str]) -> RootPackageVariables {
    RootPackageVariables {
        package_id: package_id.to_owned(),
        variables: parse_package_variables(&variables, &[])
            .unwrap_or_else(|violation| panic!("{package_id}: {violation:?}")),
        cap_fact_keys: owned(cap_fact_keys),
    }
}

/// The variable `floor`, its `max` given.
fn floor(max: i64) -> Value {
    json!({"name": "floor", "type": "integer", "title": {"en": "Keep at least"},
           "min": 0, "max": max, "step": 10, "default": 0})
}

/// A switch named `name`.
fn switch(name: &str) -> Value {
    json!({"name": name, "type": "switch", "title": {"en": "Switch"}, "default": false})
}

fn root_issue(package_id: &str, problem: StandardPackProblem, pointer: &str) -> RootVariableIssue {
    RootVariableIssue {
        package_id: package_id.to_owned(),
        issue: StandardPackRootIssue {
            file: PACKAGE_CONTROL_FILE,
            problem,
            pointer: pointer.to_owned(),
        },
    }
}

/// The names of the variables that apply, each with the packages that declare it.
fn applied(checked: &RootVariables) -> Vec<(&str, Vec<&str>)> {
    checked
        .applied
        .iter()
        .map(|variable| {
            (
                variable.name.as_str(),
                variable.packages.iter().map(String::as_str).collect(),
            )
        })
        .collect()
}

/// [`check_root_variables`] of `packages` against `goal_names` (Workflow #455 G-SLx: no longer
/// with the root's judgement of irreversibility, which these tests gave as nothing irreversible).
fn root_check(packages: &[RootPackageVariables], goal_names: &BTreeSet<&str>) -> RootVariables {
    check_root_variables(packages, goal_names)
}

#[test]
fn v1_root_check_shares_identical_declarations_and_refuses_the_rest() {
    use StandardPackProblem::{VariableCapRange, VariableConflict, VariableOutOfLimits};
    let none = BTreeSet::new();

    // Identical declarations in two packages are one variable with both packages.
    let shared = root_check(
        &[
            package("neutral.a", json!([floor(200)]), &[]),
            package("neutral.b", json!([floor(200), switch("extra")]), &[]),
            package("neutral.c", json!([switch("other")]), &[]),
        ],
        &none,
    );
    assert!(shared.issues.is_empty(), "{:?}", shared.issues);
    assert_eq!(
        applied(&shared),
        [
            ("floor", vec!["neutral.a", "neutral.b"]),
            ("extra", vec!["neutral.b"]),
            ("other", vec!["neutral.c"]),
        ]
    );

    // A different declaration of the name, a range or only the order of its members, is a
    // conflict at each declaration, and no variable of that name applies.
    let mut reordered = serde_json::Map::new();
    for member in ["name", "type", "title", "max", "min", "step", "default"] {
        reordered.insert(member.to_owned(), floor(200)[member].clone());
    }
    for (case, other) in [
        ("another max", floor(100)),
        ("another member order", Value::Object(reordered)),
    ] {
        let differing = root_check(
            &[
                package("neutral.a", json!([switch("extra"), floor(200)]), &[]),
                package("neutral.b", json!([other]), &[]),
            ],
            &none,
        );
        assert_eq!(
            differing.issues,
            [
                root_issue("neutral.a", VariableConflict, "/variables/1"),
                root_issue("neutral.b", VariableConflict, "/variables/0"),
            ],
            "{case}"
        );
        assert_eq!(
            applied(&differing),
            [("extra", vec!["neutral.a"])],
            "{case}"
        );
    }

    // A name an entry of goals.json uses for anything but its placement.
    let colliding = root_check(
        &[package("neutral.a", json!([floor(200)]), &[])],
        &BTreeSet::from(["floor"]),
    );
    assert_eq!(
        colliding.issues,
        [root_issue(
            "neutral.a",
            VariableConflict,
            "/variables/0/name"
        )]
    );
    assert!(colliding.applied.is_empty());

    // 97 names: six packages of sixteen switches and a seventh of one; the 97th does not apply.
    let mut packages = (0..6)
        .map(|package_index| {
            let switches = (0..16)
                .map(|index| switch(&format!("v{:03}", package_index * 16 + index)))
                .collect::<Vec<_>>();
            package(&format!("neutral.p{package_index}"), json!(switches), &[])
        })
        .collect::<Vec<_>>();
    packages.push(package("neutral.p6", json!([switch("v096")]), &[]));
    let crowded = root_check(&packages, &none);
    assert_eq!(
        crowded.issues,
        [root_issue(
            "neutral.p6",
            VariableOutOfLimits,
            "/variables/0/name"
        )]
    );
    assert_eq!(crowded.applied.len(), MAX_ROOT_VARIABLES);

    // An integer read as a designated effect's cap declares a max of 1000 at most; one above is
    // reported at the reading package's max, and the variable applies in no package.
    let cap = "session.dispatch.parameter.floor";
    let capped = root_check(
        &[
            package("neutral.a", json!([floor(2000)]), &[cap]),
            package("neutral.b", json!([floor(2000)]), &[]),
        ],
        &none,
    );
    assert_eq!(
        capped.issues,
        [root_issue(
            "neutral.a",
            VariableCapRange,
            "/variables/0/max"
        )]
    );
    assert!(capped.applied.is_empty());
    for (case, root) in [
        (
            "a cap of 1000",
            vec![package("neutral.a", json!([floor(1000)]), &[cap])],
        ),
        (
            "no package reads it as a cap",
            vec![package("neutral.a", json!([floor(2000)]), &[])],
        ),
    ] {
        let passed = root_check(&root, &none);
        assert!(passed.issues.is_empty(), "{case}: {:?}", passed.issues);
        assert_eq!(applied(&passed), [("floor", vec!["neutral.a"])], "{case}");
    }
}

#[test]
fn v1_provenance_lists_each_variable_by_its_default_s_origin() {
    let checked = root_check(
        &[
            package("neutral.a", declared(), &[]),
            package(
                "neutral.b",
                json!([declared()[1].clone(), switch("other")]),
                &[],
            ),
        ],
        &BTreeSet::new(),
    );
    assert!(checked.issues.is_empty(), "{:?}", checked.issues);
    let provenance = checked.provenance();
    assert_eq!(
        provenance,
        VariableOrigins {
            pack: owned(&["floor", "order", "extra", "other"]),
            model: owned(&["pick"]),
        }
    );
    assert_eq!(
        serde_json::to_value(&provenance).expect("the listing"),
        json!({"pack": ["floor", "order", "extra", "other"], "model": ["pick"]})
    );
}

// Workflow #455 G-SLx (register §1.501), where V1 fix round 1 judged irreversibility over the whole
// root: the root check takes no judgement of irreversibility, so the root of V1's fixture, in which
// `neutral.marks` marks `cap_x` irreversible and declares `mine` (which admission refused until
// 0.12.77), `neutral.provides` provides `cap_x` unmarked and declares `floor` and `extra`, a remedy
// of `neutral.blocked` lists `cap_x` and it declares `other`, and `neutral.keeps` provides `cap_y`
// and declares `extra`, as `neutral.provides` does, and `own`, has no problem: every variable
// applies, `extra` once with both packages that declare it, the provenance listing lists each by
// its default's origin and has no `not_applying` member, and the root's judgement keeps none from
// applying. A package no reader was told of is held to nothing either.
#[test]
fn gslx_root_check_applies_a_variable_of_a_package_irreversible_by_the_root() {
    let none = BTreeSet::new();
    let packages = [
        package("neutral.marks", json!([switch("mine")]), &[]),
        package(
            "neutral.provides",
            json!([floor(200), switch("extra")]),
            &[],
        ),
        package("neutral.blocked", json!([switch("other")]), &[]),
        package(
            "neutral.keeps",
            json!([switch("extra"), switch("own")]),
            &[],
        ),
    ];
    let checked = check_root_variables(&packages, &none);
    assert!(checked.issues.is_empty(), "{:?}", checked.issues);
    assert_eq!(
        applied(&checked),
        [
            ("mine", vec!["neutral.marks"]),
            ("floor", vec!["neutral.provides"]),
            ("extra", vec!["neutral.provides", "neutral.keeps"]),
            ("other", vec!["neutral.blocked"]),
            ("own", vec!["neutral.keeps"]),
        ]
    );
    let provenance = checked.provenance();
    assert_eq!(
        provenance,
        VariableOrigins {
            pack: owned(&["mine", "floor", "extra", "other", "own"]),
            model: Vec::new(),
        }
    );
    assert_eq!(
        serde_json::to_value(&provenance).expect("the listing"),
        json!({"pack": ["mine", "floor", "extra", "other", "own"], "model": []})
    );
    let judged = judge_root_variables(&packages, RootGoalDocument::Absent);
    assert_eq!(judged.checked, checked);
    assert!(judged.not_applying().is_empty());
    assert!(judged.verdicts.iter().all(RootVariableVerdict::applies));

    let untold = [package("neutral.keeps", json!([switch("own")]), &[])];
    let alone = check_root_variables(&untold, &none);
    assert!(alone.issues.is_empty(), "{:?}", alone.issues);
    assert_eq!(applied(&alone), [("own", vec!["neutral.keeps"])]);
}

// Fix round 1 (the review's suggestion 6): a designated effect's cap is not negative, so an
// `integer` read as a cap whose `min` is below 0 is `variable_cap_range` at the reading package's
// `min` only, and the variable applies in no package; the same variable read by no package as a
// cap applies.
#[test]
fn v1_a_cap_variable_with_a_negative_min_is_reported_at_its_min() {
    use StandardPackProblem::VariableCapRange;
    let none = BTreeSet::new();
    let negative = || {
        json!({"name": "floor", "type": "integer", "title": {"en": "Keep at least"},
               "min": -10, "max": 200, "step": 10, "default": 0})
    };
    let cap = "session.dispatch.parameter.floor";
    let capped = root_check(
        &[
            package("neutral.a", json!([negative()]), &[cap]),
            package("neutral.b", json!([negative()]), &[]),
        ],
        &none,
    );
    assert_eq!(
        capped.issues,
        [root_issue(
            "neutral.a",
            VariableCapRange,
            "/variables/0/min"
        )]
    );
    assert!(capped.applied.is_empty());
    let unread = root_check(&[package("neutral.a", json!([negative()]), &[])], &none);
    assert!(unread.issues.is_empty(), "{:?}", unread.issues);
    assert_eq!(applied(&unread), [("floor", vec!["neutral.a"])]);
}

// Fix round 1 (the review's suggestion 6): a cap is read from an `integer` variable only, so a
// choice, an ordered list and a switch read as a designated effect's cap are each
// `variable_cap_range` at the declaration's `type`, and none applies.
#[test]
fn v1_a_cap_read_from_a_variable_of_another_type_is_reported_at_its_type() {
    use StandardPackProblem::VariableCapRange;
    let none = BTreeSet::new();
    // The choice, the ordered list and the switch of [`declared`].
    for index in [0, 2, 3] {
        let variable = declared()[index].clone();
        let name = variable["name"].as_str().expect("a name").to_owned();
        let cap = format!("session.dispatch.parameter.{name}");
        let checked = root_check(
            &[package("neutral.a", json!([variable]), &[cap.as_str()])],
            &none,
        );
        assert_eq!(
            checked.issues,
            [root_issue(
                "neutral.a",
                VariableCapRange,
                "/variables/0/type"
            )],
            "{name}"
        );
        assert!(checked.applied.is_empty(), "{name}");
    }
}

#[test]
fn v1_a_placement_is_held_to_the_variables_the_root_declares() {
    let packages = [
        package("neutral.a", json!([floor(200), switch("extra")]), &[]),
        package("neutral.b", json!([floor(200)]), &[]),
    ];
    let variables = goal_variables(&packages);
    assert_eq!(
        variables.get("floor"),
        Some(&GoalVariable {
            block: GoalBlock::Number,
            rule_given: false,
            packages: owned(&["neutral.a", "neutral.b"]),
        })
    );
    let goals: StandardPackGoals = parse_standard_pack_goals(
        &serde_json::to_vec(&json!({
            "schema_version": "actingcommand.goals.v3",
            "game": "neutral",
            "entries": [
                {"name": "floor", "type": "variable", "block": "number"},
                {"name": "extra", "type": "variable", "block": "choice"},
                {"name": "absent", "type": "variable"}
            ]
        }))
        .expect("JSON"),
        "neutral",
    )
    .unwrap_or_else(|issues| panic!("the placements read: {issues:?}"));
    assert!(
        goals
            .entries
            .iter()
            .all(|entry| entry.kind == DeclaredGoalKind::Variable)
    );
    // The root's two packages (Workflow #455 G-SLx: by id alone).
    let mut references = GoalReferences::new(BTreeSet::from([
        "neutral.a".to_owned(),
        "neutral.b".to_owned(),
    ]));
    references.variables = variables;
    let issue = |problem, pointer: &str| StandardPackRootIssue {
        file: crate::STANDARD_PACK_GOALS_FILE,
        problem,
        pointer: pointer.to_owned(),
    };
    assert_eq!(
        check_standard_pack_goal_references(&goals, &references),
        [
            issue(StandardPackProblem::GoalBlockMismatch, "/entries/1/block"),
            issue(StandardPackProblem::GoalVariableUnknown, "/entries/2/name"),
        ]
    );
}

/// Workflow #455 TK-1: a choice only a rule gives (`given_by: "rule"`), which writes no default.
fn rule_pick() -> Value {
    json!({"name": "pick", "type": "choice", "title": {"en": "Item"},
           "options": [{"value": "item_a", "title": {"en": "Item A"}},
                       {"value": "item_b", "title": {"en": "Item B"}}],
           "given_by": "rule"})
}

// Workflow #455 TK-1 (register §1.532; contracts/package-variables.md, "A variable a rule gives"):
// a variable of each kind that writes `given_by: "rule"` and no default reads, with its word and no
// default, its declaration as written, and judges a value whole as any variable; a switch beside it
// without the member keeps its default. The reader refuses the member beside a `default` or a
// `default_origin` at that member, and a word other than `rule` (another text, a boolean) at the
// member itself, each `variables_invalid`. The root check applies such a variable, and the
// provenance listing names it in neither list.
#[test]
fn tk1_the_reader_takes_given_by_without_a_default_and_refuses_it_beside_one() {
    let ruled = json!([
        rule_pick(),
        {"name": "floor", "type": "integer", "title": {"en": "Keep at least"},
         "min": 0, "max": 200, "step": 10, "given_by": "rule"},
        {"name": "order", "type": "ordered", "title": {"en": "Order"},
         "options": [{"value": "item_a", "title": {"en": "Item A"}}],
         "min_count": 0, "max_count": 1, "given_by": "rule"},
        {"name": "extra", "type": "switch", "title": {"en": "Also the extra"}, "given_by": "rule"},
        switch("other")
    ]);
    let read = parse_package_variables(&ruled, &[])
        .unwrap_or_else(|violation| panic!("the variables a rule gives read: {violation:?}"));
    assert_eq!(
        read.iter()
            .map(|variable| (
                variable.name.as_str(),
                variable.given_by,
                variable.default_value()
            ))
            .collect::<Vec<_>>(),
        [
            ("pick", Some(GivenBy::Rule), None),
            ("floor", Some(GivenBy::Rule), None),
            ("order", Some(GivenBy::Rule), None),
            ("extra", Some(GivenBy::Rule), None),
            ("other", None, Some(json!(false))),
        ]
    );
    assert!(read[..4].iter().all(PackageVariable::rule_given));
    assert!(!read[4].rule_given());
    assert_eq!(read[0].declaration, rule_pick());
    assert_eq!(read[0].takes_whole(&json!("item_a")), Ok(json!("item_a")));
    assert_eq!(
        read[0].takes_whole(&json!("item_c")),
        Err(VariableProblem::NotAnOption)
    );
    assert_eq!(GivenBy::Rule.as_str(), "rule");

    let with_rule = |change: fn(&mut Value)| {
        let mut value = json!([rule_pick()]);
        change(&mut value);
        value
    };
    let cases = [
        (
            "a default beside it",
            with_rule(|value| value[0]["default"] = json!("item_a")),
            "/variables/0/default",
        ),
        (
            "a default_origin beside it",
            with_rule(|value| value[0]["default_origin"] = json!("model")),
            "/variables/0/default_origin",
        ),
        (
            "a word of another vocabulary",
            with_rule(|value| value[0]["given_by"] = json!("person")),
            "/variables/0/given_by",
        ),
        (
            "a word that is no text",
            with_rule(|value| value[0]["given_by"] = json!(true)),
            "/variables/0/given_by",
        ),
        (
            "an integer's default beside it",
            json!([{"name": "floor", "type": "integer", "title": {"en": "Keep at least"},
                    "min": 0, "max": 200, "default": 0, "given_by": "rule"}]),
            "/variables/0/default",
        ),
    ];
    for (case, variables, pointer) in cases {
        assert_eq!(
            parse_package_variables(&variables, &[]).map(|_| ()),
            Err(VariablesViolation {
                problem: VariablesProblem::VariablesInvalid,
                pointer: pointer.to_owned(),
            }),
            "{case}"
        );
    }

    let checked = root_check(&[package("neutral.a", ruled, &[])], &BTreeSet::new());
    assert!(checked.issues.is_empty(), "{:?}", checked.issues);
    assert_eq!(checked.applied.len(), 5);
    assert_eq!(
        checked.provenance(),
        VariableOrigins {
            pack: owned(&["other"]),
            model: Vec::new(),
        }
    );
}

// Workflow #455 TK-1: the references know a variable a rule gives as one, and a placement of it in
// `goals.json` is `goal_variable_rule_given` at its name; a placement of another variable of the
// same package is held as before.
#[test]
fn tk1_a_placement_of_a_variable_a_rule_gives_is_refused_at_its_name() {
    let packages = [package(
        "neutral.a",
        json!([rule_pick(), switch("extra")]),
        &[],
    )];
    let variables = goal_variables(&packages);
    assert_eq!(
        variables.get("pick"),
        Some(&GoalVariable {
            block: GoalBlock::Choice,
            rule_given: true,
            packages: owned(&["neutral.a"]),
        })
    );
    assert_eq!(
        variables.get("extra").map(|variable| variable.rule_given),
        Some(false)
    );
    let goals: StandardPackGoals = parse_standard_pack_goals(
        &serde_json::to_vec(&json!({
            "schema_version": "actingcommand.goals.v3",
            "game": "neutral",
            "entries": [
                {"name": "pick", "type": "variable"},
                {"name": "extra", "type": "variable"}
            ]
        }))
        .expect("JSON"),
        "neutral",
    )
    .unwrap_or_else(|issues| panic!("the placements read: {issues:?}"));
    let mut references = GoalReferences::new(BTreeSet::from(["neutral.a".to_owned()]));
    references.variables = variables;
    assert_eq!(
        check_standard_pack_goal_references(&goals, &references),
        [StandardPackRootIssue {
            file: crate::STANDARD_PACK_GOALS_FILE,
            problem: StandardPackProblem::GoalVariableRuleGiven,
            pointer: "/entries/0/name".to_owned(),
        }]
    );
    assert_eq!(
        StandardPackProblem::GoalVariableRuleGiven.as_str(),
        "goal_variable_rule_given"
    );
}
