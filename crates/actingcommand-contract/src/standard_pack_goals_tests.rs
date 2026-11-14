// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #455 G-R1 (register §1.356; contracts/standard-pack-goals.md), the reader of a game
//! root's `goals.json`: the design's neutral example reads to its typed entries in file order;
//! one entry of each of the types reads with every member its type has; and the reader is
//! strict: an unknown member, an unknown type and a type of a later version are refused, each at
//! its pointer, and (fix round 1) a bound of the shape broken is `goal_out_of_limits`. Fix round
//! 1: the references are made only from what every package of the root provides, and judge the
//! package a governed rule runs by the capabilities the whole root marks irreversible.
//!
//! Workflow #463 V1 (register §1.378; contracts/package-variables.md) replaced the four types that
//! governed packages by the placement `variable`, so these tests were adapted: the neutral example
//! places a variable where it held a `choice` entry, each of the five types reads, the four
//! replaced types are `goal_type_unknown`, and a placement is held, as a governed package was, to
//! the packages that declare its variable. A made-up game `neutral`; every id, name and text is a
//! placeholder.

use super::*;
use serde_json::{Value, json};

const GAME: &str = "neutral";

/// The design's neutral example, as the contract writes it (Workflow #463 V1: its third entry
/// places the variable `route` some package of the root declares).
const NEUTRAL_EXAMPLE: &str = r#"{"schema_version": "actingcommand.goals.v1", "game": "neutral",
 "entries": [
  {"name": "stock", "type": "heading", "title": {"en": "Stock", "zh-Hans": "储备"}},
  {"name": "points", "type": "resource", "governs": {"pool": "pool.neutral.points"},
   "title": {"en": "Points to hold", "zh-Hans": "要攒的点数"},
   "rate": {"floor_milli_per_day": 120000, "ceiling_milli_per_day": 900000}},
  {"name": "route", "type": "variable"}]}"#;

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("fixture JSON")
}

fn read(value: &Value) -> Result<StandardPackGoals, Vec<StandardPackRootIssue>> {
    parse_standard_pack_goals(&bytes(value), GAME)
}

/// Each package of `provides` with what its control declaration provides and no remedy (Workflow
/// #455 G-R2s fix round 3: the references take each package's declarations whole).
fn provided(
    provides: BTreeMap<String, BTreeMap<String, bool>>,
) -> BTreeMap<String, DeclaredCapabilities> {
    provides
        .into_iter()
        .map(|(package_id, provides)| {
            (
                package_id,
                DeclaredCapabilities {
                    provides,
                    remedies: BTreeSet::new(),
                },
            )
        })
        .collect()
}

fn issue(problem: StandardPackProblem, pointer: &str) -> StandardPackRootIssue {
    StandardPackRootIssue {
        file: STANDARD_PACK_GOALS_FILE,
        problem,
        pointer: pointer.to_owned(),
    }
}

/// A text of `(locale, text)` pairs, in this order.
fn text(pairs: &[(&str, &str)]) -> GoalText {
    pairs
        .iter()
        .map(|(locale, written)| ((*locale).to_owned(), (*written).to_owned()))
        .collect()
}

fn owned(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

/// A variable of the root shown with `block`, declared by `packages`.
fn variable(block: GoalBlock, packages: &[&str]) -> GoalVariable {
    GoalVariable {
        block,
        packages: owned(packages),
    }
}

#[test]
fn goals_neutral_example_reads_to_typed_entries() {
    let goals = parse_standard_pack_goals(NEUTRAL_EXAMPLE.as_bytes(), GAME)
        .unwrap_or_else(|issues| panic!("the neutral example reads: {issues:?}"));
    assert_eq!(goals.game, GAME);
    assert_eq!(
        goals.entries,
        vec![
            DeclaredGoal {
                name: "stock".to_owned(),
                block: Some(GoalBlock::Group),
                title: Some(text(&[("en", "Stock"), ("zh-Hans", "储备")])),
                help: None,
                kind: DeclaredGoalKind::Heading,
            },
            DeclaredGoal {
                name: "points".to_owned(),
                block: Some(GoalBlock::GoalRow),
                title: Some(text(&[("en", "Points to hold"), ("zh-Hans", "要攒的点数")])),
                help: None,
                kind: DeclaredGoalKind::Resource {
                    pool: "pool.neutral.points".to_owned(),
                    rate: Some(GoalRate {
                        floor_milli_per_day: 120_000,
                        ceiling_milli_per_day: 900_000,
                    }),
                },
            },
            DeclaredGoal {
                name: "route".to_owned(),
                block: None,
                title: None,
                help: None,
                kind: DeclaredGoalKind::Variable,
            },
        ]
    );
    // The page's order is the file's.
    assert_eq!(
        goals
            .entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.goal_type()))
            .collect::<Vec<_>>(),
        [
            ("stock", GoalType::Heading),
            ("points", GoalType::Resource),
            ("route", GoalType::Variable)
        ]
    );
    assert_eq!(
        goals.names_besides_placements(),
        BTreeSet::from(["stock", "points"])
    );
    assert_eq!(
        goals.entry("route").map(DeclaredGoal::goal_type),
        Some(GoalType::Variable)
    );
    // A root that offers the valued pool and a package that declares the variable holds every
    // reference.
    let mut references = GoalReferences::new(provided(BTreeMap::from([(
        "neutral.collect".to_owned(),
        BTreeMap::new(),
    )])));
    references
        .pools
        .insert("pool.neutral.points".to_owned(), true);
    references.variables.insert(
        "route".to_owned(),
        variable(GoalBlock::Choice, &["neutral.collect"]),
    );
    let issues = check_standard_pack_goal_references(&goals, &references);
    assert!(issues.is_empty(), "{issues:?}");
}

/// One entry of each of the five types, each with every member its type has, and a placement
/// that writes no block.
fn every_type() -> Value {
    json!({
        "schema_version": "actingcommand.goals.v1",
        "game": GAME,
        "entries": [
            {"name": "group_a", "type": "heading", "block": "group",
             "title": {"en": "Group A"}, "help": {"en": "The entries below."}},
            {"name": "about", "type": "note", "block": "text",
             "title": {"en": "About"}, "help": {"en": "Text to read."}},
            {"name": "points", "type": "resource", "block": "goal_row",
             "governs": {"pool": "pool.neutral.points"}, "title": {"en": "Points"},
             "help": {"en": "An amount to hold."},
             "rate": {"floor_milli_per_day": -5000, "ceiling_milli_per_day": 0}},
            {"name": "weight", "type": "tier", "block": "choice",
             "governs": {"rules": ["neutral.spend"]}, "title": {"en": "Weight"},
             "options": [{"value": "less", "title": {"en": "Less"}, "scale_milli": 500},
                         {"value": "normal", "title": {"en": "Normal"}, "scale_milli": 1000},
                         {"value": "more", "title": {"en": "More"}, "scale_milli": 2000}],
             "default": "normal"},
            {"name": "route", "type": "variable", "block": "choice"},
            {"name": "keep", "type": "variable"}
        ]
    })
}

#[test]
fn goals_each_type_reads_its_members() {
    let goals = read(&every_type()).unwrap_or_else(|issues| panic!("every type reads: {issues:?}"));
    let kinds = goals
        .entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.block, &entry.kind))
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        [
            (
                "group_a",
                Some(GoalBlock::Group),
                &DeclaredGoalKind::Heading
            ),
            ("about", Some(GoalBlock::Text), &DeclaredGoalKind::Note),
            (
                "points",
                Some(GoalBlock::GoalRow),
                &DeclaredGoalKind::Resource {
                    pool: "pool.neutral.points".to_owned(),
                    rate: Some(GoalRate {
                        floor_milli_per_day: -5000,
                        ceiling_milli_per_day: 0,
                    }),
                }
            ),
            (
                "weight",
                Some(GoalBlock::Choice),
                &DeclaredGoalKind::Tier {
                    rules: owned(&["neutral.spend"]),
                    options: vec![
                        GoalTierOption {
                            value: "less".to_owned(),
                            title: text(&[("en", "Less")]),
                            scale_milli: 500,
                        },
                        GoalTierOption {
                            value: "normal".to_owned(),
                            title: text(&[("en", "Normal")]),
                            scale_milli: 1000,
                        },
                        GoalTierOption {
                            value: "more".to_owned(),
                            title: text(&[("en", "More")]),
                            scale_milli: 2000,
                        },
                    ],
                    default: "normal".to_owned(),
                }
            ),
            (
                "route",
                Some(GoalBlock::Choice),
                &DeclaredGoalKind::Variable
            ),
            ("keep", None, &DeclaredGoalKind::Variable),
        ]
    );
    // Every type of version 1 once or more, in the file's order; the texts as written, and none
    // for a placement.
    assert_eq!(
        goals
            .entries
            .iter()
            .map(DeclaredGoal::goal_type)
            .collect::<BTreeSet<_>>(),
        GoalType::ALL.into_iter().collect::<BTreeSet<_>>()
    );
    assert_eq!(
        goals.entries[0].help,
        Some(text(&[("en", "The entries below.")]))
    );
    assert_eq!(goals.entries[2].title, Some(text(&[("en", "Points")])));
    assert_eq!(
        (&goals.entries[4].title, &goals.entries[4].help),
        (&None, &None)
    );
    assert_eq!(
        goals.names_besides_placements(),
        BTreeSet::from(["group_a", "about", "points", "weight"])
    );
    // A root that offers every reference holds them all.
    let mut references = GoalReferences::new(provided(BTreeMap::from([
        ("neutral.collect".to_owned(), BTreeMap::new()),
        ("neutral.spend".to_owned(), BTreeMap::new()),
    ])));
    references
        .pools
        .insert("pool.neutral.points".to_owned(), true);
    references.rules.insert(
        "neutral.spend".to_owned(),
        GoalRule {
            package_id: "neutral.spend".to_owned(),
            fixed_value: true,
            followers: Vec::new(),
        },
    );
    references.variables.insert(
        "route".to_owned(),
        variable(GoalBlock::Choice, &["neutral.collect"]),
    );
    references.variables.insert(
        "keep".to_owned(),
        variable(GoalBlock::Number, &["neutral.collect", "neutral.spend"]),
    );
    let issues = check_standard_pack_goal_references(&goals, &references);
    assert!(issues.is_empty(), "{issues:?}");
}

/// The neutral example as a value, changed by `change`.
fn with(change: impl Fn(&mut Value)) -> Value {
    let mut value: Value =
        serde_json::from_str(NEUTRAL_EXAMPLE).expect("the neutral example is JSON");
    change(&mut value);
    value
}

#[test]
fn goals_reader_is_strict() {
    use StandardPackProblem as Problem;
    let cases = [
        (
            "an unknown member of an entry",
            with(|value| value["entries"][2]["extra"] = json!(true)),
            issue(Problem::Malformed, "/entries/2/extra"),
        ),
        (
            "a member another type has",
            with(|value| {
                value["entries"][2]["rate"] =
                    json!({"floor_milli_per_day": 1, "ceiling_milli_per_day": 2});
            }),
            issue(Problem::Malformed, "/entries/2/rate"),
        ),
        (
            "an unknown member of governs",
            with(|value| value["entries"][1]["governs"]["rules"] = json!(["neutral.spend"])),
            issue(Problem::Malformed, "/entries/1/governs/rules"),
        ),
        (
            "an unknown member of the document",
            with(|value| value["extra"] = json!([])),
            issue(Problem::Malformed, "/extra"),
        ),
        (
            "a missing member",
            with(|value| value["entries"][0] = json!({"name": "stock", "type": "heading"})),
            issue(Problem::Malformed, "/entries/0/title"),
        ),
        (
            "an unknown type",
            with(|value| value["entries"][1]["type"] = json!("gauge")),
            issue(Problem::GoalTypeUnknown, "/entries/1/type"),
        ),
        (
            "a sixth type, a later version's",
            with(|value| value["entries"][1]["type"] = json!("slider")),
            issue(Problem::GoalTypeUnknown, "/entries/1/type"),
        ),
        // Workflow #463 V1: the four types that governed packages are no types of the file.
        (
            "a type the placement replaced",
            with(|value| value["entries"][2]["type"] = json!("choice")),
            issue(Problem::GoalTypeUnknown, "/entries/2/type"),
        ),
        (
            "a placement that writes a title",
            with(|value| value["entries"][2]["title"] = json!({"en": "Route"})),
            issue(Problem::Malformed, "/entries/2/title"),
        ),
        (
            "a placement whose block shows no variable",
            with(|value| value["entries"][2]["block"] = json!("goal_row")),
            issue(Problem::GoalBlockMismatch, "/entries/2/block"),
        ),
        (
            "a type that is no word",
            with(|value| value["entries"][1]["type"] = json!(3)),
            issue(Problem::Malformed, "/entries/1/type"),
        ),
        (
            "another version",
            with(|value| value["schema_version"] = json!("actingcommand.goals.v2")),
            issue(Problem::Malformed, "/schema_version"),
        ),
        (
            "another game",
            with(|value| value["game"] = json!("other")),
            issue(Problem::GameMismatch, "/game"),
        ),
        (
            "a name outside the name rule",
            with(|value| value["entries"][0]["name"] = json!("Stock")),
            issue(Problem::Malformed, "/entries/0/name"),
        ),
        (
            "a number that is no integer",
            with(|value| value["entries"][1]["rate"]["floor_milli_per_day"] = json!(1.5)),
            issue(Problem::Malformed, "/entries/1/rate/floor_milli_per_day"),
        ),
        (
            "97 entries",
            with(|value| {
                value["entries"] = (0..97)
                    .map(|index| {
                        json!({"name": format!("h{index}"), "type": "heading",
                               "title": {"en": "Heading"}})
                    })
                    .collect();
            }),
            issue(Problem::GoalOutOfLimits, "/entries"),
        ),
    ];
    for (case, value, expected) in cases {
        assert_eq!(read(&value).map(|_| ()), Err(vec![expected]), "{case}");
    }

    // A member written twice in one object, no JSON, and a file over 1 MiB.
    let repeated = br#"{"schema_version": "actingcommand.goals.v1", "game": "neutral",
        "entries": [{"name": "stock", "name": "stock", "type": "heading", "title": {"en": "Stock"}}]}"#;
    let large = vec![b' '; MAX_STANDARD_PACK_GOALS_BYTES + 1];
    for (case, bytes, expected) in [
        (
            "a repeated member",
            &repeated[..],
            issue(Problem::Malformed, "/entries/0/name"),
        ),
        ("no JSON", &b"{"[..], issue(Problem::Malformed, "")),
        (
            "over 1 MiB",
            &large[..],
            issue(Problem::GoalOutOfLimits, ""),
        ),
    ] {
        assert_eq!(
            parse_standard_pack_goals(bytes, GAME).map(|_| ()),
            Err(vec![expected]),
            "{case}"
        );
    }
}

/// G-R1 fix round 1 (items 1 to 3), with made-up packages: `neutral.discard` marks the capability
/// `cap_discard` irreversible, `neutral.reuse` provides it without the mark (and one of its own),
/// `neutral.collect` provides nothing, and `neutral.unlisted` is a package the references were not
/// told of; each runs a rule that writes a fixed value and declares a variable. A `tier` entry that
/// governs their four rules and (Workflow #463 V1, in place of the `switch` entry that governed the
/// four packages) the four placements of their variables are each `goal_governs_irreversible` for
/// the package that marks the capability and for the one that provides it unmarked,
/// `goal_package_unknown` for the package the references say nothing of, each at its pointer (a
/// placement's at its `name`), and pass `neutral.collect`. References made from no package pass no
/// rule, and know no variable.
#[test]
fn goals_references_judge_packages_and_rules_by_the_capabilities_of_the_root() {
    use StandardPackProblem as Problem;
    const PACKAGES: [&str; 4] = [
        "neutral.collect",
        "neutral.discard",
        "neutral.reuse",
        "neutral.unlisted",
    ];
    let rule_of = |package_id: &str| format!("rule.{package_id}");
    let variable_of = |package_id: &str| format!("v_{}", &package_id["neutral.".len()..]);
    let rules = PACKAGES.map(rule_of);
    let mut entries = vec![json!(
        {"name": "weight", "type": "tier", "governs": {"rules": rules},
         "title": {"en": "Weight"}, "default": "normal",
         "options": [{"value": "normal", "title": {"en": "Normal"}, "scale_milli": 1000},
                     {"value": "more", "title": {"en": "More"}, "scale_milli": 2000}]}
    )];
    entries.extend(
        PACKAGES.map(|package_id| json!({"name": variable_of(package_id), "type": "variable"})),
    );
    let goals = read(&json!({
        "schema_version": "actingcommand.goals.v1",
        "game": GAME,
        "entries": entries
    }))
    .unwrap_or_else(|issues| panic!("the declaration reads: {issues:?}"));
    let mut references = GoalReferences::new(provided(BTreeMap::from([
        ("neutral.collect".to_owned(), BTreeMap::new()),
        (
            "neutral.discard".to_owned(),
            BTreeMap::from([("cap_discard".to_owned(), true)]),
        ),
        (
            "neutral.reuse".to_owned(),
            BTreeMap::from([
                ("cap_discard".to_owned(), false),
                ("cap_reuse".to_owned(), false),
            ]),
        ),
    ])));
    for package_id in PACKAGES {
        references.rules.insert(
            rule_of(package_id),
            GoalRule {
                package_id: package_id.to_owned(),
                fixed_value: true,
                followers: Vec::new(),
            },
        );
        references.variables.insert(
            variable_of(package_id),
            variable(GoalBlock::Switch, &[package_id]),
        );
    }
    assert_eq!(
        check_standard_pack_goal_references(&goals, &references),
        [
            issue(
                Problem::GoalGovernsIrreversible,
                "/entries/0/governs/rules/1"
            ),
            issue(
                Problem::GoalGovernsIrreversible,
                "/entries/0/governs/rules/2"
            ),
            issue(Problem::GoalPackageUnknown, "/entries/0/governs/rules/3"),
            issue(Problem::GoalGovernsIrreversible, "/entries/2/name"),
            issue(Problem::GoalGovernsIrreversible, "/entries/3/name"),
            issue(Problem::GoalPackageUnknown, "/entries/4/name"),
        ]
    );

    // The only way to make references takes what the packages provide: made from none, they know
    // no rule and no variable, and nothing is passed.
    let unknown = (0..PACKAGES.len())
        .map(|j| {
            issue(
                Problem::GoalRuleUnknown,
                &format!("/entries/0/governs/rules/{j}"),
            )
        })
        .chain(
            (1..=PACKAGES.len())
                .map(|i| issue(Problem::GoalVariableUnknown, &format!("/entries/{i}/name"))),
        )
        .collect::<Vec<_>>();
    assert_eq!(
        check_standard_pack_goal_references(&goals, &GoalReferences::new(BTreeMap::new())),
        unknown
    );
}
