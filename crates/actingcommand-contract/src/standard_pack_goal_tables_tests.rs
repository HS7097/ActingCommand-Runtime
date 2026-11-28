// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #455 RR1 (register §1.535; contracts/standard-pack-goal-tables.md): the reader of a
//! game root's `goal-tables.json` and its check against the composed root. The contract's two
//! examples read to their tables; each form and limit is refused at its pointer; what the file says
//! of itself (its recipes, its prerequisites, its roles, its ladders) is held; and its pools are
//! held to the root with the references the goal check holds `goals.json` to. Made-up games
//! `neutral` and `neutral2`; every name, amount and text is a placeholder.

use super::*;
use serde_json::{Value, json};

const GAME: &str = "neutral";
const GAME_2: &str = "neutral2";

/// The contract's first example: a table whose classes carry two ladders, whose keys carry their
/// own, and a column of two subs with prerequisites (one of them no column's).
const HERO_EXAMPLE: &str = r#"{"schema_version": "actingcommand.goal-tables.v1", "game": "neutral",
 "items": {"gold": {"pool": "pool.neutral.gold"}, "exp": {"pool": "pool.neutral.exp"},
  "book_3": {"pool": "pool.neutral.book_3"}, "badge_a": {"pool": "pool.neutral.badge_a"},
  "ore_3": {"pool": "pool.neutral.ore_3"},
  "gel_3": {"pool": "pool.neutral.gel_3"},
  "ore_4": {"pool": "pool.neutral.ore_4",
            "recipe": {"makes": 1, "from": {"ore_3": 4}, "cost": {"gold": 300}}},
  "core_5": {"pool": "pool.neutral.core_5",
             "recipe": {"makes": 1, "from": {"ore_4": 1, "gel_3": 2}, "cost": {"gold": 400}}}},
 "tables": [{"name": "heroes",
  "progress": [{"fact_key": "inventory.list.neutral_roster", "key_column": "hero", "held_by": true,
                "columns": ["phase", "level"]},
               {"fact_key": "inventory.list.neutral_skills", "key_column": "hero",
                "columns": ["skill", "mastery_1", "mastery_2"]}],
  "columns": [{"name": "phase", "read": ["phase"], "per": "class"},
              {"name": "level", "read": ["phase", "level"], "per": "class", "requires": []},
              {"name": "skill", "read": ["skill"], "per": "key"},
              {"name": "mastery", "per": "sub", "subs": {"1": ["mastery_1"], "2": ["mastery_2"]},
               "requires": [{"column": "phase", "at_least": [2]}, {"column": "skill", "at_least": [4]},
                            {"condition": "side_quest"}]}],
  "classes": {"rank_6": {
     "phase": {"positions": [[0], [1], [2]],
               "steps": [{"badge_a": 4, "gold": 20000}, {"core_5": 3, "ore_4": 8, "gold": 100000}]},
     "level": {"positions": [[0, 1], [0, 2], [1, 1], [1, 2], [2, 1], [2, 2]],
               "steps": [{"exp": 100, "gold": 80}, {}, {"exp": 400, "gold": 300}, {},
                         {"exp": 900, "gold": 700}]}}},
  "keys": {"hero_a": {"class": "rank_6", "phase": {"steps": {"0": {"badge_a": 3, "gel_3": 2, "gold": 20000}}},
     "skill": {"positions": [[1], [2], [3], [4]],
               "steps": [{"book_3": 5}, {"book_3": 8, "ore_3": 4}, {"book_3": 8, "gel_3": 6}]},
     "mastery": {"1": {"positions": [[0], [1], [2]], "steps": [{"book_3": 8, "ore_4": 4}, {"book_3": 12, "core_5": 2}]},
                 "2": {"positions": [[0], [1], [2]], "steps": [{"book_3": 8, "gel_3": 5}, {"book_3": 12, "ore_4": 4}]}}},
   "hero_b": {"class": "rank_6", "skill": {"positions": [[1], [2]], "steps": [{"book_3": 5}]}}}}]}"#;

/// The contract's second example: one ladder of a class whose steps use a role each key names.
const UNIT_EXAMPLE: &str = r#"{"schema_version": "actingcommand.goal-tables.v1", "game": "neutral2",
 "items": {"coins": {"pool": "pool.neutral2.coins"}, "shard_a": {"pool": "pool.neutral2.shard_a"},
           "shard_b": {"pool": "pool.neutral2.shard_b"}},
 "tables": [{"name": "units",
  "progress": [{"fact_key": "inventory.list.neutral2_units", "key_column": "unit", "held_by": true,
                "columns": ["star", "gear"]}],
  "columns": [{"name": "rank", "read": ["star", "gear"], "per": "class"}],
  "classes": {"all": {"rank": {"positions": [[1, 0], [2, 0], [3, 0], [4, 0], [5, 1], [5, 2], [5, 3]],
     "steps": [{"@shard": 20}, {"@shard": 40}, {"@shard": 60}, {"@shard": 80, "coins": 500000},
               {"@shard": 100}, {"@shard": 120}]}}},
  "keys": {"unit_a": {"class": "all", "own": {"shard": "shard_a"}},
           "unit_b": {"class": "all", "own": {"shard": "shard_b"}}}}]}"#;

fn example(text: &str) -> Value {
    serde_json::from_str(text).expect("the example is JSON")
}

fn read(value: &Value, game: &str) -> Result<StandardPackGoalTables, Vec<StandardPackRootIssue>> {
    parse_standard_pack_goal_tables(&serde_json::to_vec(value).expect("fixture JSON"), game)
}

fn issue(problem: StandardPackProblem, pointer: &str) -> StandardPackRootIssue {
    StandardPackRootIssue {
        file: STANDARD_PACK_GOAL_TABLES_FILE,
        problem,
        pointer: pointer.to_owned(),
    }
}

/// The first example changed by `change`.
fn hero_with(change: impl Fn(&mut Value)) -> Value {
    let mut value = example(HERO_EXAMPLE);
    change(&mut value);
    value
}

/// The second example changed by `change`.
fn unit_with(change: impl Fn(&mut Value)) -> Value {
    let mut value = example(UNIT_EXAMPLE);
    change(&mut value);
    value
}

/// References whose pools are `valued`, each `true` (a goal can be put on it) or not (#455 SL2: the
/// map's [`GoalPool::Valued`] and [`GoalPool::Unvalued`]).
fn references(valued: &[(&str, bool)]) -> GoalReferences {
    let mut references = GoalReferences::new(BTreeSet::new());
    for (pool, takes_a_goal) in valued {
        let offered = if *takes_a_goal {
            GoalPool::Valued
        } else {
            GoalPool::Unvalued
        };
        references.pools.insert((*pool).to_owned(), offered);
    }
    references
}

/// Every pool of the first example, each one a goal can be put on.
const HERO_POOLS: [&str; 8] = [
    "pool.neutral.gold",
    "pool.neutral.exp",
    "pool.neutral.book_3",
    "pool.neutral.badge_a",
    "pool.neutral.ore_3",
    "pool.neutral.gel_3",
    "pool.neutral.ore_4",
    "pool.neutral.core_5",
];

fn hero_references() -> GoalReferences {
    references(&HERO_POOLS.map(|pool| (pool, true)))
}

#[test]
fn rr1_the_two_examples_read_to_their_tables() {
    let tables = read(&example(HERO_EXAMPLE), GAME)
        .unwrap_or_else(|issues| panic!("the first example reads: {issues:?}"));
    assert_eq!(tables.game, GAME);
    assert_eq!(
        tables
            .items
            .iter()
            .map(|item| item.name.as_str())
            .collect::<Vec<_>>(),
        [
            "gold", "exp", "book_3", "badge_a", "ore_3", "gel_3", "ore_4", "core_5"
        ]
    );
    assert_eq!(
        tables.item("ore_4").and_then(|item| item.recipe.clone()),
        Some(GoalRecipe {
            makes: 1,
            from: vec![("ore_3".to_owned(), 4)],
            cost: vec![("gold".to_owned(), 300)],
        })
    );
    assert_eq!(
        tables.item("gold").and_then(|item| item.recipe.clone()),
        None
    );
    assert_eq!(tables.table_names(), BTreeSet::from(["heroes".to_owned()]));
    let heroes = tables.table("heroes").expect("the table");
    assert_eq!(
        heroes
            .progress
            .iter()
            .map(|list| (
                list.fact_key.as_str(),
                list.key_column.as_str(),
                list.held_by
            ))
            .collect::<Vec<_>>(),
        [
            ("inventory.list.neutral_roster", "hero", true),
            ("inventory.list.neutral_skills", "hero", false)
        ]
    );
    assert_eq!(
        heroes
            .columns
            .iter()
            .map(|column| (column.name.as_str(), column.read.per()))
            .collect::<Vec<_>>(),
        [
            ("phase", GoalColumnPer::Class),
            ("level", GoalColumnPer::Class),
            ("skill", GoalColumnPer::Key),
            ("mastery", GoalColumnPer::Sub)
        ]
    );
    assert_eq!(
        heroes.column("level").map(|column| &column.read),
        Some(&GoalColumnRead::Class(vec![
            "phase".to_owned(),
            "level".to_owned()
        ]))
    );
    assert_eq!(
        heroes
            .column("mastery")
            .map(|column| column.requires.clone()),
        Some(vec![
            GoalRequirement::Column {
                column: "phase".to_owned(),
                at_least: vec![2],
                from: None,
            },
            GoalRequirement::Column {
                column: "skill".to_owned(),
                at_least: vec![4],
                from: None,
            },
            GoalRequirement::Condition("side_quest".to_owned()),
        ])
    );
    let rank = heroes.class("rank_6").expect("the class");
    let phase = rank.ladder("phase").expect("the class's phase ladder");
    assert_eq!(phase.positions, [vec![0], vec![1], vec![2]]);
    assert_eq!(
        phase.steps[1],
        vec![
            (GoalUse::Item("core_5".to_owned()), 3),
            (GoalUse::Item("ore_4".to_owned()), 8),
            (GoalUse::Item("gold".to_owned()), 100_000),
        ]
    );
    assert_eq!(
        rank.ladder("level").map(|ladder| ladder.steps.len()),
        Some(5)
    );
    let hero_a = heroes.key("hero_a").expect("hero_a");
    assert_eq!(hero_a.class, "rank_6");
    assert_eq!(
        hero_a.columns.first(),
        Some(&(
            "phase".to_owned(),
            GoalKeyColumn::Steps(vec![(
                0,
                vec![
                    (GoalUse::Item("badge_a".to_owned()), 3),
                    (GoalUse::Item("gel_3".to_owned()), 2),
                    (GoalUse::Item("gold".to_owned()), 20_000),
                ]
            )])
        ))
    );
    // The ladders a key has in a column: its class's, its own, one for each of its subs.
    assert_eq!(heroes.ladders_of(hero_a, "phase"), [phase]);
    assert_eq!(heroes.ladders_of(hero_a, "skill").len(), 1);
    assert_eq!(heroes.ladders_of(hero_a, "mastery").len(), 2);
    let hero_b = heroes.key("hero_b").expect("hero_b");
    assert!(heroes.ladders_of(hero_b, "mastery").is_empty());
    let held = check_standard_pack_goal_table_references(&tables, &hero_references());
    assert!(held.is_empty(), "{held:?}");

    let tables = read(&example(UNIT_EXAMPLE), GAME_2)
        .unwrap_or_else(|issues| panic!("the second example reads: {issues:?}"));
    let units = tables.table("units").expect("the table");
    let rank = units
        .class("all")
        .and_then(|class| class.ladder("rank"))
        .expect("the class's ladder");
    assert_eq!(rank.positions.len(), 7);
    assert_eq!(
        rank.steps[3],
        vec![
            (GoalUse::Role("shard".to_owned()), 80),
            (GoalUse::Item("coins".to_owned()), 500_000),
        ]
    );
    assert_eq!(
        units.key("unit_b").map(|key| key.own.clone()),
        Some(vec![("shard".to_owned(), "shard_b".to_owned())])
    );
    let pools = references(&[
        ("pool.neutral2.coins", true),
        ("pool.neutral2.shard_a", true),
        ("pool.neutral2.shard_b", true),
    ]);
    let held = check_standard_pack_goal_table_references(&tables, &pools);
    assert!(held.is_empty(), "{held:?}");
}

#[test]
fn rr1_each_form_and_limit_is_refused_at_its_pointer() {
    use StandardPackProblem as P;
    let hero_b_skill = |ladder: Value| {
        hero_with(move |value| value["tables"][0]["keys"]["hero_b"]["skill"] = ladder.clone())
    };
    let many_items = hero_with(|value| {
        for index in 0..MAX_GOAL_TABLE_ITEMS {
            value["items"][format!("spare_{index}")] = json!({});
        }
    });
    let many_tables = hero_with(|value| {
        let table = value["tables"][0].clone();
        value["tables"] = (0..=MAX_GOAL_TABLES)
            .map(|index| {
                let mut copy = table.clone();
                copy["name"] = json!(format!("table_{index}"));
                copy
            })
            .collect();
    });
    let many_keys = hero_with(|value| {
        for index in 0..MAX_GOAL_TABLE_KEYS {
            value["tables"][0]["keys"][format!("spare_{index}")] = json!({"class": "rank_6"});
        }
    });
    let cases = [
        (
            "an unknown member of the document",
            hero_with(|value| value["extra"] = json!(1)),
            GAME,
            issue(P::Malformed, "/extra"),
        ),
        (
            "another version",
            hero_with(|value| value["schema_version"] = json!("actingcommand.goal-tables.v2")),
            GAME,
            issue(P::Malformed, "/schema_version"),
        ),
        (
            "another game",
            example(HERO_EXAMPLE),
            "other",
            issue(P::GameMismatch, "/game"),
        ),
        (
            "more items than the bound",
            many_items,
            GAME,
            issue(P::TableOutOfLimits, "/items"),
        ),
        (
            "an item named like a role",
            hero_with(|value| value["items"]["@gold"] = json!({})),
            GAME,
            issue(P::Malformed, "/items/@gold"),
        ),
        (
            "a member an item does not take (the study's tier)",
            hero_with(|value| value["items"]["gold"]["tier"] = json!(3)),
            GAME,
            issue(P::Malformed, "/items/gold/tier"),
        ),
        (
            "a pool id over its bound",
            hero_with(|value| value["items"]["gold"]["pool"] = json!("p".repeat(129))),
            GAME,
            issue(P::TableOutOfLimits, "/items/gold/pool"),
        ),
        (
            "a recipe that makes nothing",
            hero_with(|value| value["items"]["ore_4"]["recipe"]["makes"] = json!(0)),
            GAME,
            issue(P::TableOutOfLimits, "/items/ore_4/recipe/makes"),
        ),
        (
            "an amount that is no integer",
            hero_with(|value| {
                value["tables"][0]["classes"]["rank_6"]["phase"]["steps"][0]["gold"] = json!(1.5);
            }),
            GAME,
            issue(P::Malformed, "/tables/0/classes/rank_6/phase/steps/0/gold"),
        ),
        (
            "more tables than the bound",
            many_tables,
            GAME,
            issue(P::TableOutOfLimits, "/tables"),
        ),
        (
            "a table named like an earlier one",
            hero_with(|value| {
                let copy = value["tables"][0].clone();
                value["tables"].as_array_mut().expect("tables").push(copy);
            }),
            GAME,
            issue(P::TableNameDuplicate, "/tables/1/name"),
        ),
        (
            "a column named like an earlier one",
            hero_with(|value| value["tables"][0]["columns"][1]["name"] = json!("phase")),
            GAME,
            issue(P::TableNameDuplicate, "/tables/0/columns/1/name"),
        ),
        (
            "a column named like a key's own member",
            hero_with(|value| value["tables"][0]["columns"][0]["name"] = json!("class")),
            GAME,
            issue(P::Malformed, "/tables/0/columns/0/name"),
        ),
        (
            "a per no column has",
            hero_with(|value| value["tables"][0]["columns"][0]["per"] = json!("row")),
            GAME,
            issue(P::Malformed, "/tables/0/columns/0/per"),
        ),
        (
            "subs on a column of per class",
            hero_with(|value| {
                value["tables"][0]["columns"][0]["subs"] = json!({"x": ["phase"]});
            }),
            GAME,
            issue(P::Malformed, "/tables/0/columns/0/subs"),
        ),
        (
            "a column that reads a column no list gives",
            hero_with(|value| value["tables"][0]["columns"][0]["read"] = json!(["rank"])),
            GAME,
            issue(P::TableColumnUnknown, "/tables/0/columns/0/read/0"),
        ),
        (
            "a reading column twice in one read",
            hero_with(|value| {
                value["tables"][0]["columns"][1]["read"] = json!(["phase", "phase"]);
            }),
            GAME,
            issue(P::TableReadingColumnDuplicate, "/tables/0/columns/1/read/1"),
        ),
        (
            "a reading column in two lists",
            hero_with(|value| {
                value["tables"][0]["progress"][1]["columns"] =
                    json!(["skill", "mastery_1", "mastery_2", "level"]);
            }),
            GAME,
            issue(
                P::TableReadingColumnDuplicate,
                "/tables/0/progress/1/columns/3",
            ),
        ),
        (
            "a fact key that is no list reading's",
            hero_with(|value| {
                value["tables"][0]["progress"][0]["fact_key"] = json!("inventory.neutral_roster");
            }),
            GAME,
            issue(P::TableListKeyInvalid, "/tables/0/progress/0/fact_key"),
        ),
        (
            "a second list that says a key is held",
            hero_with(|value| value["tables"][0]["progress"][1]["held_by"] = json!(true)),
            GAME,
            issue(P::Malformed, "/tables/0/progress/1/held_by"),
        ),
        (
            "a class ladder of a column of per key",
            hero_with(|value| {
                value["tables"][0]["classes"]["rank_6"]["skill"] =
                    json!({"positions": [[1], [2]], "steps": [{}]});
            }),
            GAME,
            issue(P::TableLadderInvalid, "/tables/0/classes/rank_6/skill"),
        ),
        (
            "a ladder over the bound of positions",
            hero_b_skill(json!({
                "positions": (0..=MAX_GOAL_TABLE_POSITIONS).map(|index| json!([index])).collect::<Vec<_>>(),
                "steps": []
            })),
            GAME,
            issue(P::TableOutOfLimits, "/tables/0/keys/hero_b/skill/positions"),
        ),
        (
            "a ladder of one position",
            hero_b_skill(json!({"positions": [[1]], "steps": []})),
            GAME,
            issue(
                P::TableLadderInvalid,
                "/tables/0/keys/hero_b/skill/positions",
            ),
        ),
        (
            "a ladder with a repeated position",
            hero_b_skill(json!({"positions": [[1], [1]], "steps": [{"book_3": 5}]})),
            GAME,
            issue(
                P::TableLadderInvalid,
                "/tables/0/keys/hero_b/skill/positions/1",
            ),
        ),
        (
            "a position not of its column's width",
            hero_b_skill(json!({"positions": [[1], [2, 0]], "steps": [{"book_3": 5}]})),
            GAME,
            issue(
                P::TableLadderInvalid,
                "/tables/0/keys/hero_b/skill/positions/1",
            ),
        ),
        (
            "a ladder with the wrong count of steps",
            hero_b_skill(json!({"positions": [[1], [2]], "steps": []})),
            GAME,
            issue(P::TableLadderInvalid, "/tables/0/keys/hero_b/skill/steps"),
        ),
        (
            "an override of a step the class's ladder does not have",
            hero_with(|value| {
                value["tables"][0]["keys"]["hero_a"]["phase"]["steps"] = json!({"2": {"gold": 1}});
            }),
            GAME,
            issue(P::TableLadderInvalid, "/tables/0/keys/hero_a/phase/steps/2"),
        ),
        (
            "an override's index with a leading zero",
            hero_with(|value| {
                value["tables"][0]["keys"]["hero_a"]["phase"]["steps"] = json!({"01": {"gold": 1}});
            }),
            GAME,
            issue(P::Malformed, "/tables/0/keys/hero_a/phase/steps/01"),
        ),
        (
            "a class the table does not have",
            hero_with(|value| value["tables"][0]["keys"]["hero_b"]["class"] = json!("rank_5")),
            GAME,
            issue(P::TableClassUnknown, "/tables/0/keys/hero_b/class"),
        ),
        (
            "a key's member that is no column",
            hero_with(|value| value["tables"][0]["keys"]["hero_b"]["speed"] = json!({})),
            GAME,
            issue(P::TableColumnUnknown, "/tables/0/keys/hero_b/speed"),
        ),
        (
            "a sub the column does not have",
            hero_with(|value| {
                value["tables"][0]["keys"]["hero_a"]["mastery"]["3"] =
                    json!({"positions": [[0], [1]], "steps": [{}]});
            }),
            GAME,
            issue(P::TableColumnUnknown, "/tables/0/keys/hero_a/mastery/3"),
        ),
        (
            "a step that names an item the file does not declare",
            hero_with(|value| {
                value["tables"][0]["classes"]["rank_6"]["phase"]["steps"][0]["gem"] = json!(1);
            }),
            GAME,
            issue(
                P::TableItemUnknown,
                "/tables/0/classes/rank_6/phase/steps/0/gem",
            ),
        ),
        (
            "more keys than the bound",
            many_keys,
            GAME,
            issue(P::TableOutOfLimits, "/tables/0/keys"),
        ),
        (
            "a key's own item the file does not declare",
            unit_with(|value| {
                value["tables"][0]["keys"]["unit_a"]["own"]["shard"] = json!("shard_c");
            }),
            GAME_2,
            issue(P::TableItemUnknown, "/tables/0/keys/unit_a/own/shard"),
        ),
    ];
    for (case, value, game, expected) in cases {
        assert_eq!(
            read(&value, game).map(|_| ()),
            Err(vec![expected]),
            "{case}"
        );
    }

    let large = vec![b' '; MAX_STANDARD_PACK_GOAL_TABLES_BYTES + 1];
    assert_eq!(
        parse_standard_pack_goal_tables(&large, GAME).map(|_| ()),
        Err(vec![issue(P::TableOutOfLimits, "")]),
        "over 4 MiB"
    );
    assert_eq!(
        parse_standard_pack_goal_tables(b"{", GAME).map(|_| ()),
        Err(vec![issue(P::Malformed, "")]),
        "no JSON"
    );
    // A member written twice in one object comes first.
    let repeated = HERO_EXAMPLE.replacen(
        r#""gold": {"pool": "pool.neutral.gold"}"#,
        r#""gold": {"pool": "pool.neutral.gold", "pool": "pool.neutral.gold"}"#,
        1,
    );
    assert_ne!(repeated, HERO_EXAMPLE, "setup: the member is written twice");
    let issues = parse_standard_pack_goal_tables(repeated.as_bytes(), GAME)
        .expect_err("a repeated member is refused");
    assert_eq!(
        issues.first(),
        Some(&issue(P::Malformed, "/items/gold/pool"))
    );
}

#[test]
fn rr1_what_the_file_says_of_itself_is_held() {
    use StandardPackProblem as P;
    let mastery_requires = |requires: Value| {
        hero_with(move |value| {
            value["tables"][0]["columns"][3]["requires"] = requires.clone();
        })
    };
    let cases = [
        (
            "an item with a recipe and no pool",
            hero_with(|value| {
                value["items"]["ore_4"]
                    .as_object_mut()
                    .expect("ore_4")
                    .remove("pool");
            }),
            vec![issue(P::TableRecipeWithoutPool, "/items/ore_4")],
        ),
        (
            "a recipe made from an item the file does not declare",
            hero_with(|value| value["items"]["ore_4"]["recipe"]["from"] = json!({"ore_2": 4})),
            vec![issue(P::TableItemUnknown, "/items/ore_4/recipe/from/ore_2")],
        ),
        (
            "a recipe cycle: each item on it, none that only leads into it",
            hero_with(|value| {
                value["items"]["ore_3"]["recipe"] = json!({"makes": 1, "from": {"ore_4": 1}});
            }),
            vec![
                issue(P::TableRecipeCycle, "/items/ore_3/recipe"),
                issue(P::TableRecipeCycle, "/items/ore_4/recipe"),
            ],
        ),
        (
            "a prerequisite of a column the table does not have",
            mastery_requires(json!([{"column": "rank", "at_least": [2]}])),
            vec![issue(
                P::TableColumnUnknown,
                "/tables/0/columns/3/requires/0/column",
            )],
        ),
        (
            "a prerequisite of a column of per sub",
            hero_with(|value| {
                value["tables"][0]["columns"][1]["requires"] =
                    json!([{"column": "mastery", "at_least": [1]}]);
            }),
            vec![issue(
                P::TableRequiresInvalid,
                "/tables/0/columns/1/requires/0/column",
            )],
        ),
        (
            "a prerequisite cycle: each column on it",
            hero_with(|value| {
                value["tables"][0]["columns"][0]["requires"] =
                    json!([{"column": "level", "at_least": [0, 1]}]);
                value["tables"][0]["columns"][1]["requires"] =
                    json!([{"column": "phase", "at_least": [1]}]);
            }),
            vec![
                issue(P::TableRequiresCycle, "/tables/0/columns/0/requires"),
                issue(P::TableRequiresCycle, "/tables/0/columns/1/requires"),
            ],
        ),
        (
            "a role a key that takes the step does not declare, at each step's member once",
            unit_with(|value| {
                value["tables"][0]["keys"]["unit_b"]
                    .as_object_mut()
                    .expect("unit_b")
                    .remove("own");
            }),
            (0..6)
                .map(|index| {
                    issue(
                        P::TableRoleUnknown,
                        &format!("/tables/0/classes/all/rank/steps/{index}/@shard"),
                    )
                })
                .collect(),
        ),
        (
            "a prerequisite's position a key with steps in the column does not hold",
            mastery_requires(json!([{"column": "phase", "at_least": [3]}])),
            vec![issue(
                P::TableRequiresInvalid,
                "/tables/0/columns/3/requires/0/at_least",
            )],
        ),
        (
            "a prerequisite's position a class with steps in the column does not hold",
            hero_with(|value| {
                value["tables"][0]["columns"][1]["requires"] =
                    json!([{"column": "phase", "at_least": [3]}]);
            }),
            vec![issue(
                P::TableRequiresInvalid,
                "/tables/0/columns/1/requires/0/at_least",
            )],
        ),
        (
            "a prerequisite's from that is no position of the requiring column",
            mastery_requires(json!([{"column": "phase", "at_least": [2], "from": [5]}])),
            vec![issue(
                P::TableRequiresInvalid,
                "/tables/0/columns/3/requires/0/from",
            )],
        ),
    ];
    for (case, value, expected) in cases {
        let game = if value["game"] == GAME { GAME } else { GAME_2 };
        assert_eq!(read(&value, game).map(|_| ()), Err(expected), "{case}");
    }

    // A prerequisite with `from` and one without, both held: the column is asked from that
    // position on, and the other for the whole column.
    let from_and_without = mastery_requires(json!([
        {"column": "phase", "at_least": [2], "from": [1]},
        {"column": "skill", "at_least": [4]}
    ]));
    let tables = read(&from_and_without, GAME)
        .unwrap_or_else(|issues| panic!("both prerequisites are held: {issues:?}"));
    assert_eq!(
        tables
            .table("heroes")
            .and_then(|table| table.column("mastery"))
            .map(|column| column.requires[0].clone()),
        Some(GoalRequirement::Column {
            column: "phase".to_owned(),
            at_least: vec![2],
            from: Some(vec![1]),
        })
    );
    // A class that has no ladder of the requiring column is not held to its prerequisite: the
    // class `rank_1` has a phase ladder without the position 2 and no level ladder, and its key
    // `hero_c` has no level ladder either, while `level` asks phase at least 2.
    let unheld_class = hero_with(|value| {
        value["tables"][0]["columns"][1]["requires"] =
            json!([{"column": "phase", "at_least": [2]}]);
        value["tables"][0]["classes"]["rank_1"] =
            json!({"phase": {"positions": [[0], [1]], "steps": [{"gold": 10}]}});
        value["tables"][0]["keys"]["hero_c"] = json!({"class": "rank_1"});
    });
    assert!(
        read(&unheld_class, GAME).is_ok(),
        "{:?}",
        read(&unheld_class, GAME)
    );
}

#[test]
fn rr1_the_pools_are_held_to_the_composed_root() {
    use StandardPackProblem as P;
    // An item without a recipe and without a pool is shown and weighs nothing: it reads.
    let tables = read(&hero_with(|value| value["items"]["chip"] = json!({})), GAME)
        .unwrap_or_else(|issues| panic!("an item without a pool reads: {issues:?}"));
    assert_eq!(
        tables.item("chip"),
        Some(&GoalTableItem {
            name: "chip".to_owned(),
            pool: None,
            recipe: None,
        })
    );
    let held = check_standard_pack_goal_table_references(&tables, &hero_references());
    assert!(held.is_empty(), "{held:?}");
    // The root declares no `badge_a` pool; `gel_3`, an item without a recipe, names a pool no
    // goal can be put on; `core_5`, an item with a recipe, names a pool that only has a reading,
    // which is taken.
    let mut root = references(&HERO_POOLS.map(|pool| {
        (
            pool,
            !matches!(pool, "pool.neutral.gel_3" | "pool.neutral.core_5"),
        )
    }));
    root.pools.remove("pool.neutral.badge_a");
    assert_eq!(
        check_standard_pack_goal_table_references(&tables, &root),
        [
            issue(P::TablePoolUnknown, "/items/badge_a/pool"),
            issue(P::TablePoolUnvalued, "/items/gel_3/pool"),
        ]
    );
    // References made from nothing know no pool.
    assert_eq!(
        check_standard_pack_goal_table_references(&tables, &references(&[]))
            .iter()
            .filter(|found| found.problem == P::TablePoolUnknown)
            .count(),
        HERO_POOLS.len()
    );
}

// Fix round 1 (ruling 4): a holding has one name in the file. The second item that names the pool
// of an earlier one (`exp` names `gold`'s) is refused `table_pool_duplicate` at its `pool`, the
// only problem; two items that name no pool name no pool twice.
#[test]
fn rr1_fix1_a_pool_two_items_name_is_refused_at_the_later_item_s_pool() {
    let named_twice = hero_with(|value| value["items"]["exp"]["pool"] = json!("pool.neutral.gold"));
    assert_eq!(
        read(&named_twice, GAME).map(|_| ()),
        Err(vec![issue(
            StandardPackProblem::TablePoolDuplicate,
            "/items/exp/pool"
        )])
    );
    let two_without = hero_with(|value| {
        value["items"]["chip"] = json!({});
        value["items"]["chip_2"] = json!({});
    });
    assert!(
        read(&two_without, GAME).is_ok(),
        "{:?}",
        read(&two_without, GAME)
    );
}

// Fix round 1 (ruling 4): a progress list whose `key_column` is also one of its own `columns`
// names that reading column twice: `table_reading_column_duplicate` at it among the columns, the
// only problem. Another list's key column among a list's columns is not that list's own.
#[test]
fn rr1_fix1_a_list_s_key_column_among_its_columns_is_refused_at_it() {
    let key_read = hero_with(|value| {
        value["tables"][0]["progress"][0]["columns"] = json!(["phase", "level", "hero"]);
    });
    assert_eq!(
        read(&key_read, GAME).map(|_| ()),
        Err(vec![issue(
            StandardPackProblem::TableReadingColumnDuplicate,
            "/tables/0/progress/0/columns/2"
        )])
    );
}

// Fix round 1 (ruling 4): two progress lists of one table that name the same `fact_key` are
// refused `table_list_key_duplicate` at the later one's `fact_key`, the only problem.
#[test]
fn rr1_fix1_two_lists_of_one_table_with_one_fact_key_are_refused_at_the_later() {
    let one_key = hero_with(|value| {
        value["tables"][0]["progress"][1]["fact_key"] = json!("inventory.list.neutral_roster");
    });
    assert_eq!(
        read(&one_key, GAME).map(|_| ()),
        Err(vec![issue(
            StandardPackProblem::TableListKeyDuplicate,
            "/tables/0/progress/1/fact_key"
        )])
    );
}

// Fix round 1 (ruling 1): the column a prerequisite requires is another column of the table and
// not one of `per: "sub"`. `skill` (per key) requiring `mastery` (per sub, its own prerequisites
// taken off so that no cycle forms) is refused `table_requires_invalid` at the prerequisite's
// `column`, the only problem; a column that requires itself reaches itself,
// `table_requires_cycle` at its `requires`, the only problem.
#[test]
fn rr1_fix1_a_prerequisite_on_a_sub_column_or_on_its_own_column_is_refused() {
    let on_sub = hero_with(|value| {
        value["tables"][0]["columns"][3]
            .as_object_mut()
            .expect("mastery")
            .remove("requires");
        value["tables"][0]["columns"][2]["requires"] =
            json!([{"column": "mastery", "at_least": [1]}]);
    });
    assert_eq!(
        read(&on_sub, GAME).map(|_| ()),
        Err(vec![issue(
            StandardPackProblem::TableRequiresInvalid,
            "/tables/0/columns/2/requires/0/column"
        )])
    );
    let on_itself = hero_with(|value| {
        value["tables"][0]["columns"][0]["requires"] =
            json!([{"column": "phase", "at_least": [1]}]);
    });
    assert_eq!(
        read(&on_itself, GAME).map(|_| ()),
        Err(vec![issue(
            StandardPackProblem::TableRequiresCycle,
            "/tables/0/columns/0/requires"
        )])
    );
}

// #455 SL2 (0.12.101; register §1.547; contracts/standard-pack-goal-tables.md, the row of
// `table_pool_unvalued`; contracts/standard-pack-goals.md, the row of `goal_pool_unvalued`): the
// references' one map says what each pool offers a goal, and each check reads it by its own rule.
// On the first example's root whose `gel_3` and `core_5` pools write `spend_line` (the map's
// `Line`): `gel_3`, an item without a recipe, is `table_pool_unvalued` at its pool, the only
// problem, as on a pool that offers nothing; `core_5`, an item with a recipe, is taken. A
// `resource` entry of `goals.json` governing such a pool is accepted; one governing a pool that
// offers nothing is `goal_pool_unvalued` at its pool, as before.
#[test]
fn sl2_a_line_s_pool_takes_a_resource_entry_and_no_need_lands_on_it() {
    use StandardPackProblem as P;
    let tables = read(&example(HERO_EXAMPLE), GAME)
        .unwrap_or_else(|issues| panic!("the first example reads: {issues:?}"));
    let mut root = hero_references();
    for pool in ["pool.neutral.gel_3", "pool.neutral.core_5"] {
        root.pools.insert(pool.to_owned(), GoalPool::Line);
    }
    assert_eq!(
        check_standard_pack_goal_table_references(&tables, &root),
        [issue(P::TablePoolUnvalued, "/items/gel_3/pool")]
    );
    root.pools
        .insert("pool.neutral.gold".to_owned(), GoalPool::Unvalued);
    let governing = |pool: &str| {
        let document = json!({"schema_version": "actingcommand.goals.v3", "game": GAME,
            "entries": [{"name": "saved", "type": "resource", "governs": {"pool": pool},
                         "title": {"en": "Saved"}}]});
        crate::parse_standard_pack_goals(&serde_json::to_vec(&document).expect("JSON"), GAME)
            .unwrap_or_else(|issues| panic!("the goals read: {issues:?}"))
    };
    let on_line =
        crate::check_standard_pack_goal_references(&governing("pool.neutral.gel_3"), &root);
    assert!(on_line.is_empty(), "{on_line:?}");
    assert_eq!(
        crate::check_standard_pack_goal_references(&governing("pool.neutral.gold"), &root),
        [StandardPackRootIssue {
            file: crate::STANDARD_PACK_GOALS_FILE,
            problem: P::GoalPoolUnvalued,
            pointer: "/entries/0/governs/pool".to_owned(),
        }]
    );
}

/// Workflow #455 RR4a (register §1.546; contracts/standard-pack-goal-tables.md, "Names for
/// people"): the contract's example of `names` on the first example's table, as the contract
/// prints it. It names every kind; it says the positions of a column of width 1 (`phase`), of one
/// of width 2 with two prefixes and the fallback of the empty prefix (`level`) and of a column of
/// `per: "sub"` (`mastery`), whose sub `2` has a list of its own and whose sub `1` has none.
const NAMES_EXAMPLE: &str = r#"{"items": {"gold": {"en": "Gold", "fr": "Or"}, "core_5": {"en": "Core"}},
 "tables": {"heroes": {
  "keys": {"hero_a": {"en": "First hero"}},
  "columns": {"phase": {"en": "Phase"}, "level": {"en": "Level"}, "mastery": {"en": "Mastery"}},
  "subs": {"mastery": {"1": {"en": "First mastery"}, "2": {"en": "Second mastery"}}},
  "classes": {"rank_6": {"en": "Class six"}},
  "conditions": {"side_quest": {"en": "The side quest"}},
  "positions": {
   "phase": [{"prefix": [], "text": {"en": "Phase {0}"}}],
   "level": [{"prefix": [], "text": {"en": "Phase {0}, level {1}"}},
             {"prefix": [0], "text": {"en": "Level {1}", "fr": "Niveau {1}"}},
             {"prefix": [2, 2], "text": {"en": "Top level"}}],
   "mastery": [{"prefix": [], "text": {"en": "Mastery {0}"}}]},
  "sub_positions": {"mastery": {"2": [{"prefix": [2], "text": {"en": "Second mastery done"}}]}}}}}"#;

/// RR4a: the first example with the contract's `names`.
fn hero_named() -> Value {
    let mut value = example(HERO_EXAMPLE);
    value["names"] = example(NAMES_EXAMPLE);
    value
}

/// RR4a: a localized text of `languages`, each a language and its string, in their order.
fn localized(languages: &[(&str, &str)]) -> GoalText {
    languages
        .iter()
        .map(|(language, text)| ((*language).to_owned(), (*text).to_owned()))
        .collect()
}

/// RR4a: the first example whose `names`, changed by `change`, is refused with the one problem
/// `problem` at `pointer`.
#[track_caller]
fn assert_names_refused(change: impl Fn(&mut Value), problem: StandardPackProblem, pointer: &str) {
    let mut value = hero_named();
    change(&mut value["names"]);
    assert_eq!(
        read(&value, GAME).map(|_| ()),
        Err(vec![issue(problem, pointer)]),
        "{pointer}"
    );
}

/// RR4a: `count` segments of distinct prefixes of one element, for a column of width 1 or more.
fn segments_of(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({"prefix": [index], "text": {"en": "Level"}}))
            .collect(),
    )
}

// Workflow #455 RR4a (0.12.100; register §1.546), tests 1 and 4: the contract's first example with
// its example of `names`, which names every kind, reads, every name and every list kept as
// written; what the file declares besides reads to what the same file without the member reads
// to, and that file's names are none.
#[test]
fn rr4a_the_first_example_with_names_reads_and_its_tables_are_those_of_the_file_without() {
    let named = read(&hero_named(), GAME)
        .unwrap_or_else(|issues| panic!("the first example with names reads: {issues:?}"));
    let plain = read(&example(HERO_EXAMPLE), GAME)
        .unwrap_or_else(|issues| panic!("the first example reads: {issues:?}"));
    assert_eq!(plain.names, GoalTableNames::default());
    assert_eq!(named.game, plain.game);
    assert_eq!(named.items, plain.items);
    assert_eq!(named.tables, plain.tables);

    let names = &named.names;
    assert_eq!(
        names
            .items
            .iter()
            .map(|(item, _)| item.as_str())
            .collect::<Vec<_>>(),
        ["gold", "core_5"]
    );
    assert_eq!(
        names.item("gold"),
        Some(&localized(&[("en", "Gold"), ("fr", "Or")]))
    );
    assert_eq!(names.item("exp"), None, "an item without a name");
    assert!(names.table("other").is_none());
    let heroes = names.table("heroes").expect("the table's names");
    assert_eq!(
        heroes.key("hero_a"),
        Some(&localized(&[("en", "First hero")]))
    );
    assert_eq!(heroes.key("hero_b"), None, "a key without a name");
    assert_eq!(heroes.column("level"), Some(&localized(&[("en", "Level")])));
    assert_eq!(
        heroes.sub("mastery", "2"),
        Some(&localized(&[("en", "Second mastery")]))
    );
    assert_eq!(
        heroes.class("rank_6"),
        Some(&localized(&[("en", "Class six")]))
    );
    assert_eq!(
        heroes.condition("side_quest"),
        Some(&localized(&[("en", "The side quest")]))
    );
    let level = heroes.segments("level").expect("the level's segments");
    assert_eq!(
        level
            .iter()
            .map(|segment| segment.prefix.clone())
            .collect::<Vec<_>>(),
        [vec![], vec![0], vec![2, 2]]
    );
    assert_eq!(
        level[1].text,
        localized(&[("en", "Level {1}"), ("fr", "Niveau {1}")])
    );
    assert_eq!(heroes.segments("phase").map(<[_]>::len), Some(1));
    assert_eq!(heroes.segments("mastery").map(<[_]>::len), Some(1));
    assert_eq!(heroes.segments("skill"), None, "a column without a list");
    assert_eq!(heroes.sub_segments("mastery", "1"), None);
    assert_eq!(
        heroes
            .sub_segments("mastery", "2")
            .map(|list| list[0].prefix.clone()),
        Some(vec![2])
    );
}

// RR4a, test 2 and addendum 1: each refusal of `names` at its pointer, the only problem, in the
// reader's existing words. A name of an item, a table, a key, a column, a sub, a class or a
// condition the file does not declare; a sub named under a column that is not of `per: "sub"`;
// an unknown member of a table's names and of `names`; a prefix longer than the column's width (a
// column of `per: "sub"`: its subs' width), two segments of one prefix, a placeholder at the
// width, a lone `{` and a lone `}`: `malformed`. 65 segments and none, and a text the localized
// form refuses (a string of 129 bytes, one over a title's 128): `table_out_of_limits`. Of
// `sub_positions`: a column not of `per: "sub"` and a sub the column does not have, `malformed` at
// the key; a prefix longer than the sub's own width at it. 64 segments and a string of 128 bytes
// read.
#[test]
fn rr4a_each_name_of_nothing_declared_and_each_bad_segment_is_refused_at_its_pointer() {
    use StandardPackProblem as P;
    assert_names_refused(
        |names| names["items"]["silver"] = json!({"en": "Silver"}),
        P::Malformed,
        "/names/items/silver",
    );
    assert_names_refused(
        |names| names["tables"]["villains"] = json!({}),
        P::Malformed,
        "/names/tables/villains",
    );
    assert_names_refused(
        |names| names["tables"]["heroes"]["keys"]["hero_z"] = json!({"en": "Last hero"}),
        P::Malformed,
        "/names/tables/heroes/keys/hero_z",
    );
    assert_names_refused(
        |names| names["tables"]["heroes"]["columns"]["grade"] = json!({"en": "Grade"}),
        P::Malformed,
        "/names/tables/heroes/columns/grade",
    );
    assert_names_refused(
        |names| names["tables"]["heroes"]["subs"]["mastery"]["3"] = json!({"en": "Third"}),
        P::Malformed,
        "/names/tables/heroes/subs/mastery/3",
    );
    assert_names_refused(
        |names| names["tables"]["heroes"]["classes"]["rank_5"] = json!({"en": "Class five"}),
        P::Malformed,
        "/names/tables/heroes/classes/rank_5",
    );
    assert_names_refused(
        |names| names["tables"]["heroes"]["conditions"]["main_quest"] = json!({"en": "Main"}),
        P::Malformed,
        "/names/tables/heroes/conditions/main_quest",
    );
    assert_names_refused(
        |names| names["tables"]["heroes"]["subs"]["phase"] = json!({"1": {"en": "First"}}),
        P::Malformed,
        "/names/tables/heroes/subs/phase",
    );
    assert_names_refused(
        |names| names["tables"]["heroes"]["rows"] = json!({}),
        P::Malformed,
        "/names/tables/heroes/rows",
    );
    assert_names_refused(
        |names| names["titles"] = json!({}),
        P::Malformed,
        "/names/titles",
    );
    assert_names_refused(
        |names| {
            names["tables"]["heroes"]["positions"]["phase"] =
                json!([{"prefix": [0, 1], "text": {"en": "Phase"}}]);
        },
        P::Malformed,
        "/names/tables/heroes/positions/phase/0/prefix",
    );
    assert_names_refused(
        |names| {
            names["tables"]["heroes"]["positions"]["mastery"] =
                json!([{"prefix": [0, 1], "text": {"en": "Mastery"}}]);
        },
        P::Malformed,
        "/names/tables/heroes/positions/mastery/0/prefix",
    );
    assert_names_refused(
        |names| {
            names["tables"]["heroes"]["positions"]["level"] = json!([
                {"prefix": [0], "text": {"en": "Level"}},
                {"prefix": [0], "text": {"en": "Low level"}}
            ]);
        },
        P::Malformed,
        "/names/tables/heroes/positions/level/1/prefix",
    );
    for template in ["Phase {1}", "Phase {0", "Phase 0}", "Phase {x}"] {
        assert_names_refused(
            |names| {
                names["tables"]["heroes"]["positions"]["phase"][0]["text"]["en"] = json!(template)
            },
            P::Malformed,
            "/names/tables/heroes/positions/phase/0/text/en",
        );
    }
    for count in [MAX_GOAL_POSITION_SEGMENTS + 1, 0] {
        assert_names_refused(
            |names| names["tables"]["heroes"]["positions"]["level"] = segments_of(count),
            P::TableOutOfLimits,
            "/names/tables/heroes/positions/level",
        );
    }
    assert_names_refused(
        |names| names["tables"]["heroes"]["columns"]["phase"]["en"] = json!("p".repeat(129)),
        P::TableOutOfLimits,
        "/names/tables/heroes/columns/phase/en",
    );
    assert_names_refused(
        |names| {
            names["tables"]["heroes"]["sub_positions"]["level"] =
                json!({"1": [{"prefix": [], "text": {"en": "Level"}}]});
        },
        P::Malformed,
        "/names/tables/heroes/sub_positions/level",
    );
    assert_names_refused(
        |names| {
            names["tables"]["heroes"]["sub_positions"]["mastery"]["3"] =
                json!([{"prefix": [], "text": {"en": "Third"}}]);
        },
        P::Malformed,
        "/names/tables/heroes/sub_positions/mastery/3",
    );
    assert_names_refused(
        |names| {
            names["tables"]["heroes"]["sub_positions"]["mastery"]["2"][0]["prefix"] = json!([2, 0]);
        },
        P::Malformed,
        "/names/tables/heroes/sub_positions/mastery/2/0/prefix",
    );

    let mut bounds = hero_named();
    bounds["names"]["tables"]["heroes"]["positions"]["level"] =
        segments_of(MAX_GOAL_POSITION_SEGMENTS);
    bounds["names"]["tables"]["heroes"]["columns"]["phase"]["en"] = json!("p".repeat(128));
    assert_eq!(read(&bounds, GAME).map(|_| ()), Ok(()));
}

// RR4a, test 3 and addendum 1: a position is said by the segment whose prefix is its first
// elements, the longest such (the empty prefix the fallback), its placeholders filled with the
// decimal numbers, in every language that segment writes (one it does not write is not in the
// answer); a column without a list and a placeholder past the position say nothing. A sub with a
// list of its own is said by it alone (a position none of its prefixes begins has no name); a sub
// without one by its column's list.
#[test]
fn rr4a_a_position_is_said_by_its_longest_prefix_in_every_language_of_its_segment() {
    let named = read(&hero_named(), GAME)
        .unwrap_or_else(|issues| panic!("the first example with names reads: {issues:?}"));
    let heroes = named.names.table("heroes").expect("the table's names");
    let say = |column: &str, sub: Option<&str>, position: &[i64]| {
        say_goal_position(heroes, column, sub, position)
    };
    assert_eq!(
        say("level", None, &[0, 2]),
        Some(localized(&[("en", "Level 2"), ("fr", "Niveau 2")])),
        "the longest prefix"
    );
    assert_eq!(
        say("level", None, &[1, 1]),
        Some(localized(&[("en", "Phase 1, level 1")])),
        "the empty prefix"
    );
    assert_eq!(
        say("level", None, &[2, 2]),
        Some(localized(&[("en", "Top level")]))
    );
    assert_eq!(
        say("level", None, &[2, 1]),
        Some(localized(&[("en", "Phase 2, level 1")]))
    );
    assert_eq!(
        say("phase", None, &[1]),
        Some(localized(&[("en", "Phase 1")]))
    );
    assert_eq!(
        say("phase", None, &[-3]),
        Some(localized(&[("en", "Phase -3")]))
    );
    assert_eq!(say("level", None, &[1]), None, "a placeholder past it");
    assert_eq!(say("skill", None, &[3]), None, "a column without a list");
    assert_eq!(
        say("mastery", Some("1"), &[1]),
        Some(localized(&[("en", "Mastery 1")])),
        "a sub without a list of its own"
    );
    assert_eq!(
        say("mastery", None, &[0]),
        Some(localized(&[("en", "Mastery 0")]))
    );
    assert_eq!(
        say("mastery", Some("2"), &[2]),
        Some(localized(&[("en", "Second mastery done")])),
        "a sub's own list"
    );
    assert_eq!(
        say("mastery", Some("2"), &[1]),
        None,
        "a sub's own list alone"
    );
}
