// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #455 G-R1 (register §1.356; contracts/standard-pack-goals.md), the reader of a game
//! root's `goals.json`: the design's neutral example reads to its typed entries in file order;
//! one entry of each of the types reads with every member its type has; and the reader is
//! strict: an unknown member, an unknown type and a type of a later version are refused, each at
//! its pointer, and (fix round 1) a bound of the shape broken is `goal_out_of_limits`. Fix round
//! 1: the references are made only from every package of the root, and hold the package a
//! governed rule runs to them (Workflow #455 G-SLx: no longer judged by the capabilities the
//! whole root marks irreversible).
//!
//! Workflow #463 V1 (register §1.378; contracts/package-variables.md) replaced the four types that
//! governed packages by the placement `variable`, so these tests were adapted: the neutral example
//! places a variable where it held a `choice` entry, each of the five types reads, the four
//! replaced types are `goal_type_unknown`, and a placement is held, as a governed package was, to
//! the packages that declare its variable being packages of the root. V1 fix round 3 made the
//! document of the five types
//! version 2: the fixtures write `actingcommand.goals.v2`, "another version" is a later one, and a
//! version 1 document is `goal_version_retired` and read no further. Workflow #455 RR1 (register
//! §1.535) made the document version 3, version 2's types and `records`: the fixtures write
//! `actingcommand.goals.v3` (their entries as they were), one entry of each type includes a
//! `records` entry, "another version" is version 4, and a version 2 document is retired as version
//! 1 is. A made-up game `neutral`; every id, name and text is a placeholder.

use super::*;
use serde_json::{Value, json};

const GAME: &str = "neutral";

/// The design's neutral example, as the contract writes it (Workflow #463 V1: its third entry
/// places the variable `route` some package of the root declares; version 2 since its fix round
/// 3; Workflow #455 RR1: version 3, its entries as they were, which read as version 2 read them).
const NEUTRAL_EXAMPLE: &str = r#"{"schema_version": "actingcommand.goals.v3", "game": "neutral",
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

/// The packages of a root, by id (Workflow #455 G-SLx: the references take the packages alone,
/// where they took what each package's declarations say until 0.12.77).
fn packages(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
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
        rule_given: false,
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
    let mut references = GoalReferences::new(packages(&["neutral.collect"]));
    references
        .pools
        .insert("pool.neutral.points".to_owned(), GoalPool::Valued);
    references.variables.insert(
        "route".to_owned(),
        variable(GoalBlock::Choice, &["neutral.collect"]),
    );
    let issues = check_standard_pack_goal_references(&goals, &references);
    assert!(issues.is_empty(), "{issues:?}");
}

/// One entry of each of the six types of version 3 (Workflow #455 RR1 added `records`), each with
/// every member its type has, and a placement that writes no block.
fn every_type() -> Value {
    json!({
        "schema_version": "actingcommand.goals.v3",
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
            {"name": "keep", "type": "variable"},
            {"name": "raise", "type": "records", "block": "record_list", "table": "heroes",
             "max_records": 16, "title": {"en": "Whom to raise"}, "help": {"en": "Records."}}
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
            (
                "raise",
                Some(GoalBlock::RecordList),
                &DeclaredGoalKind::Records {
                    table: "heroes".to_owned(),
                    max_records: 16,
                }
            ),
        ]
    );
    // Every type of version 3 once or more, in the file's order; the texts as written, and none
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
        BTreeSet::from(["group_a", "about", "points", "weight", "raise"])
    );
    // A root that offers every reference holds them all (Workflow #455 RR1: a table of its
    // `goal-tables.json` too).
    let mut references = GoalReferences::new(packages(&["neutral.collect", "neutral.spend"]));
    references.tables = Some(BTreeSet::from(["heroes".to_owned()]));
    references
        .pools
        .insert("pool.neutral.points".to_owned(), GoalPool::Valued);
    references.rules.insert(
        "neutral.spend".to_owned(),
        GoalRule {
            package_id: "neutral.spend".to_owned(),
            fixed_value: true,
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
            "a seventh type, a later version's",
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
        // V1 fix round 3: version 2 is the file's; a version this build does not know yet keeps
        // `malformed`, a word other than version 1's `goal_version_retired`. Workflow #455 RR1:
        // version 3 is the file's, and the later version is version 4.
        (
            "a later version",
            with(|value| value["schema_version"] = json!("actingcommand.goals.v4")),
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
    let repeated = br#"{"schema_version": "actingcommand.goals.v3", "game": "neutral",
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

/// Workflow #463 V1 fix round 3 (register §1.378; the coordinator's ruling 2 on the design's
/// questions, release 0.12.60 having shipped version 1 with its eight types): this build reads
/// version 2 only. A version 1 document as release 0.12.60 read it, that release's neutral example
/// (its third entry a `choice` that governs a package), is refused with one problem,
/// `goal_version_retired` at `/schema_version`, and read no further: its `choice` entry is not
/// judged, nor, in the same document of another game, its `game`. The version 2 neutral example
/// labelled version 1 is the same one problem. (A version this build does not know yet keeps
/// `malformed`, `goals_reader_is_strict`.)
#[test]
fn v1_a_version_1_document_is_refused_goal_version_retired() {
    let retired = || -> Result<(), Vec<StandardPackRootIssue>> {
        Err(vec![issue(
            StandardPackProblem::GoalVersionRetired,
            "/schema_version",
        )])
    };
    // That release's neutral example, of the game `game`.
    let released = |game: &str| {
        json!({
            "schema_version": "actingcommand.goals.v1",
            "game": game,
            "entries": [
                {"name": "stock", "type": "heading", "title": {"en": "Stock", "zh-Hans": "储备"}},
                {"name": "points", "type": "resource", "governs": {"pool": "pool.neutral.points"},
                 "title": {"en": "Points to hold", "zh-Hans": "要攒的点数"},
                 "rate": {"floor_milli_per_day": 120000, "ceiling_milli_per_day": 900000}},
                {"name": "route", "type": "choice", "governs": {"packages": ["neutral.collect"]},
                 "title": {"en": "Route", "zh-Hans": "路线"}, "default": "a",
                 "options": [{"value": "a", "title": {"en": "Route A"}},
                             {"value": "b", "title": {"en": "Route B"}}]}
            ]
        })
    };
    assert_eq!(
        read(&released(GAME)).map(|_| ()),
        retired(),
        "the neutral example of release 0.12.60"
    );
    assert_eq!(
        read(&released("other")).map(|_| ()),
        retired(),
        "read no further: its game is not judged"
    );
    let relabelled = with(|value| value["schema_version"] = json!("actingcommand.goals.v1"));
    assert_eq!(
        read(&relabelled).map(|_| ()),
        retired(),
        "version 2's entries labelled version 1"
    );
}

/// G-R1 fix round 1 (items 1 to 3), with made-up packages: `neutral.collect`, `neutral.discard`
/// and `neutral.reuse` are packages of the root, and `neutral.unlisted` is a package the
/// references were not told of; each runs a rule that writes a fixed value and declares a
/// variable. Workflow #455 G-SLx (register §1.501): the references are made from the root's
/// packages alone, and whether a package can do something irreversible is no reference any more
/// (until 0.12.77 `neutral.discard` marked a capability irreversible and `neutral.reuse` provided
/// it unmarked, and both were `goal_governs_irreversible`). A `tier` entry that governs their four
/// rules and the four placements of their variables are `goal_package_unknown` for the package
/// the references say nothing of, each at its pointer (a placement's at its `name`), and nothing
/// else: the three packages of the root pass. References made from no package pass no rule, and
/// know no variable.
#[test]
fn gslx_goals_references_hold_rules_and_placements_to_the_packages_of_the_root() {
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
        "schema_version": "actingcommand.goals.v3",
        "game": GAME,
        "entries": entries
    }))
    .unwrap_or_else(|issues| panic!("the declaration reads: {issues:?}"));
    let mut references = GoalReferences::new(packages(&PACKAGES[..3]));
    for package_id in PACKAGES {
        references.rules.insert(
            rule_of(package_id),
            GoalRule {
                package_id: package_id.to_owned(),
                fixed_value: true,
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
            issue(Problem::GoalPackageUnknown, "/entries/0/governs/rules/3"),
            issue(Problem::GoalPackageUnknown, "/entries/4/name"),
        ]
    );

    // The only way to make references takes the root's packages: made from none, they know no
    // rule and no variable, and nothing is passed.
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
        check_standard_pack_goal_references(&goals, &GoalReferences::new(BTreeSet::new())),
        unknown
    );
}

/// Workflow #455 RR1: a version 3 document of one `records` entry beside a heading, the entry
/// changed by `change`.
fn with_records(change: impl Fn(&mut Value)) -> Value {
    let mut value = json!({
        "schema_version": "actingcommand.goals.v3",
        "game": GAME,
        "entries": [
            {"name": "stock", "type": "heading", "title": {"en": "Stock"}},
            {"name": "raise", "type": "records", "table": "heroes", "max_records": 16,
             "title": {"en": "Whom to raise"}}
        ]
    });
    change(&mut value["entries"][1]);
    value
}

/// Workflow #455 RR1 (register §1.535; contracts/standard-pack-goals.md, "The records entry"):
/// a `records` entry reads with its table and its bound, its block the type's base block; each of
/// its members and its bound is refused at its pointer; and its table is held to the tables of the
/// root's accepted `goal-tables.json`: known it passes, unknown (a root without the file too) it is
/// `goal_table_unknown` at its `table`, and while the root's file is refused it is not judged.
#[test]
fn rr1_a_records_entry_reads_and_is_held_to_the_root_s_tables() {
    use StandardPackProblem as Problem;
    let goals = read(&with_records(|_| {}))
        .unwrap_or_else(|issues| panic!("a records entry reads: {issues:?}"));
    let raise = goals.entry("raise").expect("the records entry");
    assert_eq!(
        (raise.goal_type(), raise.block, &raise.kind),
        (
            GoalType::Records,
            Some(GoalBlock::RecordList),
            &DeclaredGoalKind::Records {
                table: "heroes".to_owned(),
                max_records: 16,
            }
        )
    );
    assert_eq!(raise.title, Some(text(&[("en", "Whom to raise")])));
    for bound in [1, MAX_GOAL_RECORDS] {
        assert!(
            read(&with_records(|entry| entry["max_records"] = json!(bound))).is_ok(),
            "{bound} records"
        );
    }
    let cases = [
        (
            "no record",
            with_records(|entry| entry["max_records"] = json!(0)),
            issue(Problem::GoalOutOfLimits, "/entries/1/max_records"),
        ),
        (
            "more records than the bound",
            with_records(|entry| entry["max_records"] = json!(MAX_GOAL_RECORDS + 1)),
            issue(Problem::GoalOutOfLimits, "/entries/1/max_records"),
        ),
        (
            "a bound that is no whole number",
            with_records(|entry| entry["max_records"] = json!(-1)),
            issue(Problem::Malformed, "/entries/1/max_records"),
        ),
        (
            "no table",
            with_records(|entry| {
                entry.as_object_mut().expect("the entry").remove("table");
            }),
            issue(Problem::Malformed, "/entries/1/table"),
        ),
        (
            "a table outside a name's form",
            with_records(|entry| entry["table"] = json!("Heroes")),
            issue(Problem::Malformed, "/entries/1/table"),
        ),
        (
            "another type's block",
            with_records(|entry| entry["block"] = json!("choice")),
            issue(Problem::GoalBlockMismatch, "/entries/1/block"),
        ),
        (
            "a member another type has",
            with_records(|entry| entry["governs"] = json!({"pool": "pool.neutral.points"})),
            issue(Problem::Malformed, "/entries/1/governs"),
        ),
    ];
    for (case, value, expected) in cases {
        assert_eq!(read(&value).map(|_| ()), Err(vec![expected]), "{case}");
    }

    let mut references = GoalReferences::new(BTreeSet::new());
    assert_eq!(
        check_standard_pack_goal_references(&goals, &references),
        [issue(Problem::GoalTableUnknown, "/entries/1/table")],
        "a root without the file"
    );
    references.tables = Some(BTreeSet::from(["units".to_owned()]));
    assert_eq!(
        check_standard_pack_goal_references(&goals, &references),
        [issue(Problem::GoalTableUnknown, "/entries/1/table")],
        "a table the file does not have"
    );
    references.tables = Some(BTreeSet::from(["heroes".to_owned(), "units".to_owned()]));
    assert!(
        check_standard_pack_goal_references(&goals, &references).is_empty(),
        "a table of the root's file"
    );
    references.tables = None;
    assert!(
        check_standard_pack_goal_references(&goals, &references).is_empty(),
        "a refused file: the table is not judged"
    );
}

/// Workflow #455 RR1 (ruling 1): a version 2 document is retired as version 1 is, one problem at
/// `/schema_version`, read no further (its game neither); the same entries written as version 3
/// read to what `goals_neutral_example_reads_to_typed_entries` pins, which is what version 2 read
/// them to.
#[test]
fn rr1_version_2_is_retired_and_version_3_reads_its_entries() {
    let retired = || -> Result<(), Vec<StandardPackRootIssue>> {
        Err(vec![issue(
            StandardPackProblem::GoalVersionRetired,
            "/schema_version",
        )])
    };
    let version_2 = with(|value| value["schema_version"] = json!("actingcommand.goals.v2"));
    assert_eq!(read(&version_2).map(|_| ()), retired(), "version 2");
    let other_game = with(|value| {
        value["schema_version"] = json!("actingcommand.goals.v2");
        value["game"] = json!("other");
    });
    assert_eq!(
        read(&other_game).map(|_| ()),
        retired(),
        "read no further: its game is not judged"
    );
    let version_3 = parse_standard_pack_goals(NEUTRAL_EXAMPLE.as_bytes(), GAME)
        .unwrap_or_else(|issues| panic!("version 3 reads: {issues:?}"));
    assert_eq!(
        version_3
            .entries
            .iter()
            .map(DeclaredGoal::goal_type)
            .collect::<Vec<_>>(),
        [GoalType::Heading, GoalType::Resource, GoalType::Variable]
    );
}
