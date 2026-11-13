// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #455 G-R1 (register §1.350; contracts/standard-pack-goals.md), the reader of a game
//! root's `goals.json`: the design's neutral example reads to its typed entries in file order;
//! one entry of each of the eight types reads with every member its type has; and the reader is
//! strict: an unknown member, an unknown type and a type of a later version are refused, each at
//! its pointer, and (fix round 1) a bound of the shape broken is `goal_out_of_limits`. Fix round
//! 1: the references are made only from what every package of the root provides, and judge a
//! governed package, and the package a governed rule runs, by the capabilities the whole root
//! marks irreversible. A made-up game `neutral`; every id, name and text is a placeholder.

use super::*;
use serde_json::{Value, json};

const GAME: &str = "neutral";

/// The design's neutral example, as the design writes it.
const NEUTRAL_EXAMPLE: &str = r#"{"schema_version": "actingcommand.goals.v1", "game": "neutral",
 "entries": [
  {"name": "stock", "type": "heading", "title": {"en": "Stock", "zh-Hans": "储备"}},
  {"name": "points", "type": "resource", "governs": {"pool": "pool.neutral.points"},
   "title": {"en": "Points to hold", "zh-Hans": "要攒的点数"},
   "rate": {"floor_milli_per_day": 120000, "ceiling_milli_per_day": 900000}},
  {"name": "route", "type": "choice", "governs": {"packages": ["neutral.collect"]},
   "title": {"en": "Route", "zh-Hans": "路线"}, "default": "a",
   "options": [{"value": "a", "title": {"en": "Route A"}}, {"value": "b", "title": {"en": "Route B"}}]}]}"#;

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("fixture JSON")
}

fn read(value: &Value) -> Result<StandardPackGoals, Vec<StandardPackRootIssue>> {
    parse_standard_pack_goals(&bytes(value), GAME)
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

fn option(value: &str, title: &str) -> GoalOption {
    GoalOption {
        value: value.to_owned(),
        title: text(&[("en", title)]),
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
                block: GoalBlock::Group,
                title: text(&[("en", "Stock"), ("zh-Hans", "储备")]),
                help: None,
                kind: DeclaredGoalKind::Heading,
            },
            DeclaredGoal {
                name: "points".to_owned(),
                block: GoalBlock::GoalRow,
                title: text(&[("en", "Points to hold"), ("zh-Hans", "要攒的点数")]),
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
                block: GoalBlock::Choice,
                title: text(&[("en", "Route"), ("zh-Hans", "路线")]),
                help: None,
                kind: DeclaredGoalKind::Choice {
                    packages: owned(&["neutral.collect"]),
                    options: vec![option("a", "Route A"), option("b", "Route B")],
                    default: "a".to_owned(),
                },
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
            ("route", GoalType::Choice)
        ]
    );
    assert_eq!(
        goals.governed_packages(),
        BTreeSet::from(["neutral.collect"])
    );
    assert!(goals.tables().is_empty());
    assert_eq!(
        goals.entry("route").map(DeclaredGoal::goal_type),
        Some(GoalType::Choice)
    );
    // A root that offers the valued pool and the package holds every reference.
    let mut references = GoalReferences::new(BTreeMap::from([(
        "neutral.collect".to_owned(),
        BTreeMap::new(),
    )]));
    references
        .pools
        .insert("pool.neutral.points".to_owned(), true);
    references.tables = Some(BTreeMap::new());
    let issues = check_standard_pack_goal_references(&goals, &references);
    assert!(issues.is_empty(), "{issues:?}");
}

/// One entry of each of the eight types, each with every member its type has.
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
            {"name": "keep", "type": "integer", "block": "number",
             "governs": {"packages": ["neutral.collect", "neutral.spend"]},
             "title": {"en": "Points to keep"}, "help": {"en": "Kept after a run."},
             "min": 0, "max": 200, "step": 10, "unit": {"en": "points", "zh-Hans": "点"},
             "default": 50},
            {"name": "wait", "type": "integer", "governs": {"packages": ["neutral.collect"]},
             "title": {"en": "Time between two runs"},
             "min": 600000, "max": 43200000, "unit": "minute", "default": 600000},
            {"name": "route", "type": "choice", "block": "choice",
             "governs": {"packages": ["neutral.collect"]}, "title": {"en": "Route"},
             "options": [{"value": "a", "title": {"en": "Route A"}},
                         {"value": "b", "title": {"en": "Route B"}},
                         {"value": "c", "title": {"en": "Route C"}}],
             "default": "c"},
            {"name": "enabled", "type": "switch", "block": "switch",
             "governs": {"packages": ["neutral.collect"]}, "title": {"en": "Collect"},
             "default": true},
            {"name": "order", "type": "ordered", "block": "ordered_list",
             "governs": {"packages": ["neutral.collect"]}, "title": {"en": "First first"},
             "options": [{"value": "m01", "title": {"en": "First"}},
                         {"value": "m02", "title": {"en": "Second"}},
                         {"value": "m03", "title": {"en": "Third"}}],
             "min_count": 0, "max_count": 2, "default": ["m02"]},
            {"name": "members", "type": "ordered", "governs": {"packages": ["neutral.collect"]},
             "title": {"en": "Members"},
             "from_table": {"table": "neutral_members",
                            "names": {"m01": {"en": "First member"}}},
             "min_count": 1, "max_count": 3, "default": ["m01", "m03"]},
            {"name": "weight", "type": "tier", "block": "choice",
             "governs": {"rules": ["neutral.spend"]}, "title": {"en": "Weight"},
             "options": [{"value": "less", "title": {"en": "Less"}, "scale_milli": 500},
                         {"value": "normal", "title": {"en": "Normal"}, "scale_milli": 1000},
                         {"value": "more", "title": {"en": "More"}, "scale_milli": 2000}],
             "default": "normal"}
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
            ("group_a", GoalBlock::Group, &DeclaredGoalKind::Heading),
            ("about", GoalBlock::Text, &DeclaredGoalKind::Note),
            (
                "points",
                GoalBlock::GoalRow,
                &DeclaredGoalKind::Resource {
                    pool: "pool.neutral.points".to_owned(),
                    rate: Some(GoalRate {
                        floor_milli_per_day: -5000,
                        ceiling_milli_per_day: 0,
                    }),
                }
            ),
            (
                "keep",
                GoalBlock::Number,
                &DeclaredGoalKind::Integer {
                    packages: owned(&["neutral.collect", "neutral.spend"]),
                    min: 0,
                    max: 200,
                    step: 10,
                    unit: Some(GoalUnit::Text(text(&[("en", "points"), ("zh-Hans", "点")]))),
                    default: 50,
                }
            ),
            (
                "wait",
                GoalBlock::Number,
                &DeclaredGoalKind::Integer {
                    packages: owned(&["neutral.collect"]),
                    min: 600_000,
                    max: 43_200_000,
                    step: 1,
                    unit: Some(GoalUnit::Time(GoalTimeUnit::Minute)),
                    default: 600_000,
                }
            ),
            (
                "route",
                GoalBlock::Choice,
                &DeclaredGoalKind::Choice {
                    packages: owned(&["neutral.collect"]),
                    options: vec![
                        option("a", "Route A"),
                        option("b", "Route B"),
                        option("c", "Route C")
                    ],
                    default: "c".to_owned(),
                }
            ),
            (
                "enabled",
                GoalBlock::Switch,
                &DeclaredGoalKind::Switch {
                    packages: owned(&["neutral.collect"]),
                    default: true,
                }
            ),
            (
                "order",
                GoalBlock::OrderedList,
                &DeclaredGoalKind::Ordered {
                    packages: owned(&["neutral.collect"]),
                    values: GoalValues::Options(vec![
                        option("m01", "First"),
                        option("m02", "Second"),
                        option("m03", "Third")
                    ]),
                    min_count: 0,
                    max_count: 2,
                    default: owned(&["m02"]),
                }
            ),
            (
                "members",
                GoalBlock::OrderedList,
                &DeclaredGoalKind::Ordered {
                    packages: owned(&["neutral.collect"]),
                    values: GoalValues::Table {
                        table: "neutral_members".to_owned(),
                        names: vec![("m01".to_owned(), text(&[("en", "First member")]))],
                    },
                    min_count: 1,
                    max_count: 3,
                    default: owned(&["m01", "m03"]),
                }
            ),
            (
                "weight",
                GoalBlock::Choice,
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
        ]
    );
    // Every type of version 1 once or more, in the file's order; the texts as written.
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
    assert_eq!(goals.entries[2].title, text(&[("en", "Points")]));
    assert_eq!(
        goals.governed_packages(),
        BTreeSet::from(["neutral.collect", "neutral.spend"])
    );
    assert_eq!(goals.tables(), BTreeSet::from(["neutral_members"]));
    // A root that offers every reference holds them all.
    let mut references = GoalReferences::new(BTreeMap::from([
        ("neutral.collect".to_owned(), BTreeMap::new()),
        ("neutral.spend".to_owned(), BTreeMap::new()),
    ]));
    references
        .pools
        .insert("pool.neutral.points".to_owned(), true);
    references.rules.insert(
        "neutral.spend".to_owned(),
        GoalRule {
            package_id: "neutral.spend".to_owned(),
            fixed_value: true,
        },
    );
    references.tables = Some(BTreeMap::from([(
        "neutral_members".to_owned(),
        owned(&["m01", "m02", "m03"]),
    )]));
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
            with(|value| value["entries"][2]["governs"]["pool"] = json!("pool.neutral.points")),
            issue(Problem::Malformed, "/entries/2/governs/pool"),
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
            "a ninth type, a later version's",
            with(|value| value["entries"][1]["type"] = json!("slider")),
            issue(Problem::GoalTypeUnknown, "/entries/1/type"),
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
/// told of; each runs a rule that writes a fixed value. A `switch` entry that governs the four
/// packages and a `tier` entry that governs their four rules are each `goal_governs_irreversible`
/// for the package that marks the capability and for the one that provides it unmarked,
/// `goal_package_unknown` for the package the references say nothing of, each at its pointer, and
/// pass `neutral.collect`. References made from no package pass no package and no rule.
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
    let rules = PACKAGES.map(rule_of);
    let goals = read(&json!({
        "schema_version": "actingcommand.goals.v1",
        "game": GAME,
        "entries": [
            {"name": "enabled", "type": "switch", "governs": {"packages": PACKAGES},
             "title": {"en": "Enabled"}, "default": true},
            {"name": "weight", "type": "tier", "governs": {"rules": rules},
             "title": {"en": "Weight"}, "default": "normal",
             "options": [{"value": "normal", "title": {"en": "Normal"}, "scale_milli": 1000},
                         {"value": "more", "title": {"en": "More"}, "scale_milli": 2000}]}
        ]
    }))
    .unwrap_or_else(|issues| panic!("the declaration reads: {issues:?}"));
    let mut references = GoalReferences::new(BTreeMap::from([
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
    ]));
    for package_id in PACKAGES {
        references.rules.insert(
            rule_of(package_id),
            GoalRule {
                package_id: package_id.to_owned(),
                fixed_value: true,
            },
        );
    }
    assert_eq!(
        check_standard_pack_goal_references(&goals, &references),
        [
            issue(
                Problem::GoalGovernsIrreversible,
                "/entries/0/governs/packages/1"
            ),
            issue(
                Problem::GoalGovernsIrreversible,
                "/entries/0/governs/packages/2"
            ),
            issue(Problem::GoalPackageUnknown, "/entries/0/governs/packages/3"),
            issue(
                Problem::GoalGovernsIrreversible,
                "/entries/1/governs/rules/1"
            ),
            issue(
                Problem::GoalGovernsIrreversible,
                "/entries/1/governs/rules/2"
            ),
            issue(Problem::GoalPackageUnknown, "/entries/1/governs/rules/3"),
        ]
    );

    // The only way to make references takes what the packages provide: made from none, they know
    // no package and no rule, and nothing is passed.
    let unknown = (0..PACKAGES.len())
        .map(|j| {
            issue(
                Problem::GoalPackageUnknown,
                &format!("/entries/0/governs/packages/{j}"),
            )
        })
        .chain((0..PACKAGES.len()).map(|j| {
            issue(
                Problem::GoalRuleUnknown,
                &format!("/entries/1/governs/rules/{j}"),
            )
        }))
        .collect::<Vec<_>>();
    assert_eq!(
        check_standard_pack_goal_references(&goals, &GoalReferences::new(BTreeMap::new())),
        unknown
    );
}
