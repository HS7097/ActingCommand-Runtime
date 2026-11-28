// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #455 RR3 (0.12.95; register §1.538; the coordinator's rulings 1 to 11 of 11-28 in its
// work order, on the read-only study of derived training targets; contracts/standard-pack-goal-
// tables.md, "What is computed"; contracts/resource-targets.md, "Needs computed from the
// records"), the host's half. Before each evaluation the host computes, for each instance, what
// the characters its stored records name still need, with one pure function
// (`crate::goal_needs::instance_needs`), and gives it to the evaluator in memory:
// - the numbers of the study's answer 五 that are the host's: an item of need 10 and holding 4 with
//   a recipe of 4 for 1 and a cost of 300 gives 24 of the lower item and 1800 of the cost; an item
//   without a recipe and without a pool is shown and weighs nothing; the set's freshness is its
//   lists' (p1);
// - the walk: a target behind the progress asks nothing; a prerequisite raises the required column
//   only for the steps that ask it (`from`); two records that need one item are summed before it
//   is taken apart; a key's own item; a condition is named (p2);
// - pending and known: each cause adds nothing with its own note; a `held_by` list without the
//   key is not held; a record the root does not declare is not applied with the edit's word; a
//   holding not known adds nothing below its item; a pending column never raises a need (p3);
// - two instances of one root compute apart (p4);
// - in a host on a manual clock: the needs are computed from the published records, lists and
//   tables, and the evaluation pushes the producer of the pool the need lands on (h1); an intent
//   taken before a change of a progress list, of the records or of the tables is refused
//   `policy_facts_stale`, the needs' identity having moved (h2).
//
// The tables, keys, items and pools are made up; times are fixed Unix milliseconds, and nothing
// sleeps.

use super::*;
use crate::PolicyTrigger;
use crate::goal_needs::{NeedsSources, RecordRoots, RecordsRoot, instance_needs};
use crate::goal_records::{GOAL_RECORDS_SCHEMA_VERSION, StoredGoalRecords};
use actingcommand_contract::{
    FactScalar, GOAL_RECORDS_FACT_KEY, GoalProblem, ResourceTargetPendingReason, RuntimeChannel,
    StandardPackGoalTables, parse_standard_pack_goal_tables,
};
use actingcommand_policy::{
    InstanceRecordNeeds, NeedHolding, PolicyEvaluation, PoolNeed, PoolNeedKind,
    ProgressListUnknown, RecordNeedsNote, RecordRef, TaskDecision,
};
use serde_json::{Value, json};

const ALIAS: &str = POLICY_INSTANCE_ALIAS;
const NOW: u64 = POLICY_NOW_UNIX_MS;
/// When the fixture's lists are read: ten seconds before the fixture's time.
const READ_AT: u64 = POLICY_NOW_UNIX_MS - 10_000;
/// How long a list read stays fresh.
const FRESH_MS: u64 = 60_000;
/// The table's `held_by` list, which gives `grade`.
const ROSTER: &str = "inventory.list.fixture_roster";
/// The table's second list, which gives `skill`.
const SKILLS: &str = "inventory.list.fixture_skills";
/// The records entry, and the table it names.
const ENTRY: &str = "raise";
const TABLE: &str = "units";
/// The game the fixture's tables files are written for.
const TABLES_GAME: &str = "neutral";
const SHA: &str = "fixture-tables-a";

// ---- The pure computation

/// What the fixture's computation reads besides the records and the root: lists by key (fresh
/// until their expiry at the fixture's time) and holdings by pool (none: not known).
struct Sources {
    lists: BTreeMap<String, FactRecord>,
    holdings: BTreeMap<String, NeedHolding>,
}

impl NeedsSources for Sources {
    fn list(&self, fact_key: &str) -> Option<&FactRecord> {
        self.lists.get(fact_key)
    }

    fn list_freshness(
        &self,
        _fact_key: &str,
        _observed_at_unix_ms: u64,
        expires_at_unix_ms: Option<u64>,
    ) -> Result<Option<u64>, ProgressListUnknown> {
        if expires_at_unix_ms.is_some_and(|expires| NOW > expires) {
            Err(ProgressListUnknown::FactExpired)
        } else {
            Ok(expires_at_unix_ms)
        }
    }

    fn holding(&self, pool_id: &str) -> NeedHolding {
        self.holdings
            .get(pool_id)
            .copied()
            .unwrap_or(NeedHolding::Unknown(None))
    }
}

fn own_scope() -> FactScope {
    FactScope::Instance {
        instance_id: ALIAS.to_owned(),
    }
}

/// The list `fact_key` of `rows`, read at [`READ_AT`], of the expiry `expires_at_unix_ms` and the
/// confidence `confidence_milli`.
fn list_at(
    fact_key: &str,
    rows: Vec<BTreeMap<String, FactScalar>>,
    expires_at_unix_ms: u64,
    confidence_milli: u16,
) -> FactRecord {
    let mut fact = stored_fact(
        own_scope(),
        fact_key,
        ContractFactValue::RecordList(rows),
        &format!("snapshot:rr3:{fact_key}"),
        Vec::new(),
    );
    fact.observed_at_unix_ms = READ_AT;
    fact.expires_at_unix_ms = Some(expires_at_unix_ms);
    fact.confidence_milli = confidence_milli;
    fact
}

/// The list `fact_key` of `rows`, read at [`READ_AT`], fresh for [`FRESH_MS`], at confidence 900.
fn list(fact_key: &str, rows: Vec<BTreeMap<String, FactScalar>>) -> FactRecord {
    list_at(fact_key, rows, READ_AT + FRESH_MS, 900)
}

/// A row `{unit, <column>}` of a list.
fn row(unit: &str, column: &str, value: FactScalar) -> BTreeMap<String, FactScalar> {
    BTreeMap::from([
        ("unit".to_owned(), FactScalar::String(unit.to_owned())),
        (column.to_owned(), value),
    ])
}

/// The roster ([`ROSTER`]) of `units`, each with its grade.
fn roster(units: &[(&str, i64)]) -> FactRecord {
    list(
        ROSTER,
        units
            .iter()
            .map(|(unit, grade)| row(unit, "grade", FactScalar::Integer(*grade)))
            .collect(),
    )
}

/// The skills list ([`SKILLS`]) of `units`, each with its skill.
fn skills(units: &[(&str, FactScalar)]) -> FactRecord {
    list(
        SKILLS,
        units
            .iter()
            .map(|(unit, skill)| row(unit, "skill", skill.clone()))
            .collect(),
    )
}

fn text(value: &str) -> FactScalar {
    FactScalar::String(value.to_owned())
}

/// The header row of `alias`'s `session.goal_records` record.
fn header(alias: &str) -> BTreeMap<String, FactScalar> {
    BTreeMap::from([
        ("row".to_owned(), text("header")),
        (
            "schema_version".to_owned(),
            text(GOAL_RECORDS_SCHEMA_VERSION),
        ),
        ("instance".to_owned(), text(alias)),
    ])
}

/// A record row of `entry`, `key` and the canonical `to`, set at the fixture's time on the CLI.
fn record_row(entry: &str, key: &str, to: &str) -> BTreeMap<String, FactScalar> {
    BTreeMap::from([
        ("row".to_owned(), text("record")),
        ("entry".to_owned(), text(entry)),
        ("key".to_owned(), text(key)),
        ("to".to_owned(), text(to)),
        (
            "set_unix_ms".to_owned(),
            FactScalar::Integer(i64::try_from(NOW).expect("the fixture's time")),
        ),
        ("channel".to_owned(), text(RuntimeChannel::Cli.as_str())),
    ])
}

/// `alias`'s stored records `given` (entry, key, to), read by the goal records' own reader.
fn stored(alias: &str, given: &[(&str, &str, &str)]) -> StoredGoalRecords {
    let mut rows = vec![header(alias)];
    rows.extend(
        given
            .iter()
            .map(|(entry, key, to)| record_row(entry, key, to)),
    );
    let read = StoredGoalRecords::read(Some(rows.as_slice()), alias);
    assert!(
        matches!(&read, StoredGoalRecords::Read(taken) if taken.len() == given.len()),
        "the fixture's records read: {read:?}"
    );
    read
}

fn tables(document: &Value) -> StandardPackGoalTables {
    parse_standard_pack_goal_tables(document.to_string().as_bytes(), TABLES_GAME)
        .unwrap_or_else(|issues| panic!("the fixture's tables read: {issues:?}"))
}

/// The root whose records entry [`ENTRY`] names the table [`TABLE`] of `parsed`, of the sha256
/// `sha`.
fn root(parsed: StandardPackGoalTables, sha: &str) -> RecordsRoot {
    RecordsRoot {
        entries: BTreeMap::from([(ENTRY.to_owned(), TABLE.to_owned())]),
        tables: Some((Arc::new(parsed), sha.to_owned())),
    }
}

/// The tables of test 1: `top` is made 1 for 4 `low` and 300 `coin`; a key's one grade step needs
/// 10 `top` and 2 `seal`, an item of no pool.
fn recipe_tables() -> Value {
    json!({
        "schema_version": "actingcommand.goal-tables.v1",
        "game": TABLES_GAME,
        "items": {
            "top": {"pool": "pool.top", "recipe": {"makes": 1, "from": {"low": 4}, "cost": {"coin": 300}}},
            "low": {"pool": "pool.low"},
            "coin": {"pool": "pool.coin"},
            "seal": {}
        },
        "tables": [{
            "name": TABLE,
            "progress": [{"fact_key": ROSTER, "key_column": "unit", "held_by": true, "columns": ["grade"]}],
            "columns": [{"name": "grade", "read": ["grade"], "per": "class"}],
            "classes": {"all": {"grade": {"positions": [[0], [1]], "steps": [{"top": 10, "seal": 2}]}}},
            "keys": {"unit_a": {"class": "all"}}
        }]
    })
}

/// The tables of the walk: a class's grade ladder `[0] [1] [2]` (100 and 200 `gold`) read from the
/// roster, and each key's skill ladder `[1] [2] [3]` (5 `book`, then 7 `book` and 3 of its own
/// shard) read from the skills list, whose steps that arrive at `[3]` or beyond ask grade `[2]`,
/// and every step a condition. `book` is made 2 for 3 `paper`.
fn walk_tables() -> Value {
    let skill =
        json!({"positions": [[1], [2], [3]], "steps": [{"book": 5}, {"book": 7, "@shard": 3}]});
    json!({
        "schema_version": "actingcommand.goal-tables.v1",
        "game": TABLES_GAME,
        "items": {
            "gold": {"pool": "pool.gold"},
            "book": {"pool": "pool.book", "recipe": {"makes": 2, "from": {"paper": 3}}},
            "paper": {"pool": "pool.paper"},
            "shard_a": {"pool": "pool.shard_a"},
            "shard_b": {"pool": "pool.shard_b"}
        },
        "tables": [{
            "name": TABLE,
            "progress": [
                {"fact_key": ROSTER, "key_column": "unit", "held_by": true, "columns": ["grade"]},
                {"fact_key": SKILLS, "key_column": "unit", "columns": ["skill"]}
            ],
            "columns": [
                {"name": "grade", "read": ["grade"], "per": "class"},
                {"name": "skill", "read": ["skill"], "per": "key", "requires": [
                    {"column": "grade", "at_least": [2], "from": [3]},
                    {"condition": "side_story"}
                ]}
            ],
            "classes": {"all": {"grade": {"positions": [[0], [1], [2]], "steps": [{"gold": 100}, {"gold": 200}]}}},
            "keys": {
                "unit_a": {"class": "all", "own": {"shard": "shard_a"}, "skill": skill},
                "unit_b": {"class": "all", "own": {"shard": "shard_b"}, "skill": skill},
                "unit_c": {"class": "all", "own": {"shard": "shard_a"}, "skill": skill}
            }
        }]
    })
}

/// The needs of [`ALIAS`]'s records `given` over the walk's root, reading `lists` and `holdings`.
fn walked(
    given: &[(&str, &str, &str)],
    lists: Vec<FactRecord>,
    holdings: Vec<(&str, NeedHolding)>,
) -> InstanceRecordNeeds {
    let sources = Sources {
        lists: lists
            .into_iter()
            .map(|read| (read.key.clone(), read))
            .collect(),
        holdings: holdings
            .into_iter()
            .map(|(pool, holding)| (pool.to_owned(), holding))
            .collect(),
    };
    instance_needs(
        &stored(ALIAS, given),
        NOW,
        Some(&root(tables(&walk_tables()), SHA)),
        &sources,
    )
    .expect("an instance with records has needs")
}

fn known(held: u64) -> NeedHolding {
    NeedHolding::Known {
        held,
        fresh_until_unix_ms: None,
    }
}

fn pools(entries: &[(&str, u64, PoolNeedKind)]) -> BTreeMap<String, PoolNeed> {
    entries
        .iter()
        .map(|(pool, need, kind)| {
            (
                (*pool).to_owned(),
                PoolNeed {
                    need: *need,
                    kind: *kind,
                },
            )
        })
        .collect()
}

fn record(key: &str) -> RecordRef {
    RecordRef {
        entry: ENTRY.to_owned(),
        key: key.to_owned(),
    }
}

/// One case of test 3: the stored record (entry, key, to), the lists read, and the one note the
/// needs must hold.
struct Case {
    name: &'static str,
    given: (&'static str, &'static str, &'static str),
    lists: Vec<FactRecord>,
    note: RecordNeedsNote,
}

// Test 1, the study's numbers (ruling 4). A record that raises a key one grade needs 10 of `top`,
// held 4: the holding covers 4 of it, the 6 left are made, 6 makings of 4 `low` and 300 `coin`
// each: 24 `low` and 1800 `coin`, which land on their pools; `seal`, of no recipe and no pool, is
// noted and weighs nothing. The set names its tables' sha256, and its freshness ends with the
// roster's expiry.
#[test]
fn rr3_p1_an_item_with_a_recipe_is_covered_by_its_holding_and_the_rest_is_made() {
    let sources = Sources {
        lists: BTreeMap::from([(ROSTER.to_owned(), roster(&[("unit_a", 0)]))]),
        holdings: BTreeMap::from([("pool.top".to_owned(), known(4))]),
    };
    let needs = instance_needs(
        &stored(ALIAS, &[(ENTRY, "unit_a", r#"{"grade":[1]}"#)]),
        NOW,
        Some(&root(tables(&recipe_tables()), SHA)),
        &sources,
    )
    .expect("an instance with records has needs");
    assert_eq!(
        needs.pools(),
        &pools(&[
            ("pool.coin", 1_800, PoolNeedKind::Lands),
            ("pool.low", 24, PoolNeedKind::Lands),
            ("pool.top", 4, PoolNeedKind::Covered),
        ])
    );
    assert_eq!(
        needs.notes(),
        [RecordNeedsNote::ItemUnweighed {
            item: "seal".to_owned(),
            need: 2,
        }]
    );
    assert_eq!(needs.tables_sha256(), Some(SHA));
    assert_eq!(needs.fresh_until_unix_ms(), Some(READ_AT + FRESH_MS));
    assert_eq!(needs.records_observed_at_unix_ms(), NOW);
    // An instance without stored records, or with a header alone, has no needs at all.
    assert!(
        instance_needs(
            &StoredGoalRecords::Absent,
            NOW,
            Some(&root(tables(&recipe_tables()), SHA)),
            &sources
        )
        .is_none()
    );
    assert!(
        instance_needs(
            &stored(ALIAS, &[]),
            NOW,
            Some(&root(tables(&recipe_tables()), SHA)),
            &sources
        )
        .is_none()
    );
}

// Test 2, the walk (rulings 2 and 4). `unit_a` from skill 1 to 2: its step arrives at `[2]`, before
// the prerequisite's `from`, so grade is not raised: 5 `book`. `unit_b` from skill 1 to 3 with a
// grade target of `[0]`, its own position: the step that arrives at `[3]` asks grade `[2]`, so the
// walk takes grade from 0 to 2 as well (300 `gold`), and skill needs 12 `book` and 3 of its own
// shard. `unit_c`'s grade target is behind its progress: nothing. The two records' 17 `book` are
// summed before taking apart: held 4, the 13 left are 7 makings of 2, 21 `paper` (taken record by
// record it would be 15). Each record whose skill takes a step names the condition.
#[test]
fn rr3_p2_the_walk_counts_what_the_targets_ask_from_the_progress_and_sums_before_taking_apart() {
    let needs = walked(
        &[
            (ENTRY, "unit_a", r#"{"skill":[2]}"#),
            (ENTRY, "unit_b", r#"{"grade":[0],"skill":[3]}"#),
            (ENTRY, "unit_c", r#"{"grade":[1]}"#),
        ],
        vec![
            roster(&[("unit_a", 0), ("unit_b", 0), ("unit_c", 2)]),
            skills(&[
                ("unit_a", FactScalar::Integer(1)),
                ("unit_b", FactScalar::Integer(1)),
                ("unit_c", FactScalar::Integer(1)),
            ]),
        ],
        vec![("pool.book", known(4))],
    );
    assert_eq!(
        needs.pools(),
        &pools(&[
            ("pool.book", 4, PoolNeedKind::Covered),
            ("pool.gold", 300, PoolNeedKind::Lands),
            ("pool.paper", 21, PoolNeedKind::Lands),
            ("pool.shard_b", 3, PoolNeedKind::Lands),
        ])
    );
    let condition = |key: &str| RecordNeedsNote::Condition {
        record: record(key),
        column: "skill".to_owned(),
        condition: "side_story".to_owned(),
    };
    assert_eq!(needs.notes(), [condition("unit_a"), condition("unit_b")]);
}

// Test 3, not known is pending and adds nothing (ruling 3), each cause with its own note; the
// `held_by` list without the key is not held, which is known; a record the root does not declare
// is not applied with the word a goal edit of it would be refused with now; a holding not known
// adds nothing below its item (ruling 4); and a pending column never raises a need.
#[test]
fn rr3_p3_each_cause_adds_nothing_with_its_own_note_and_the_need_is_a_lower_bound() {
    let skill_2 = (ENTRY, "unit_a", r#"{"skill":[2]}"#);
    let roster_a = || roster(&[("unit_a", 0)]);
    let skills_a = |skill: FactScalar| skills(&[("unit_a", skill)]);
    let pending = |column: Option<&str>, fact_key: &str, reason| RecordNeedsNote::ProgressUnknown {
        record: record("unit_a"),
        column: column.map(str::to_owned),
        fact_key: fact_key.to_owned(),
        reason,
    };
    let position_unknown = RecordNeedsNote::PositionUnknown {
        record: record("unit_a"),
        column: "skill".to_owned(),
    };
    let not_applied = |entry: &str, key: &str, problem| RecordNeedsNote::NotApplied {
        record: Some(RecordRef {
            entry: entry.to_owned(),
            key: key.to_owned(),
        }),
        problem,
    };
    let reading_missing = RecordNeedsNote::ReadingMissing {
        record: record("unit_a"),
        column: "skill".to_owned(),
        fact_key: SKILLS.to_owned(),
        reading: "skill".to_owned(),
    };
    let grade_0 = || vec![row("unit_a", "grade", FactScalar::Integer(0))];
    let cases = [
        Case {
            name: "no roster",
            given: skill_2,
            lists: vec![skills_a(FactScalar::Integer(1))],
            note: pending(None, ROSTER, ProgressListUnknown::FactMissing),
        },
        Case {
            name: "an expired roster",
            given: skill_2,
            lists: vec![
                list_at(ROSTER, grade_0(), NOW - 1, 900),
                skills_a(FactScalar::Integer(1)),
            ],
            note: pending(None, ROSTER, ProgressListUnknown::FactExpired),
        },
        Case {
            name: "a roster of confidence 0",
            given: skill_2,
            lists: vec![
                list_at(ROSTER, grade_0(), READ_AT + FRESH_MS, 0),
                skills_a(FactScalar::Integer(1)),
            ],
            note: pending(None, ROSTER, ProgressListUnknown::FactLowConfidence),
        },
        Case {
            name: "a roster without the key",
            given: skill_2,
            lists: vec![roster(&[("unit_b", 0)]), skills_a(FactScalar::Integer(1))],
            note: RecordNeedsNote::NotHeld {
                record: record("unit_a"),
            },
        },
        Case {
            name: "no skills list",
            given: skill_2,
            lists: vec![roster_a()],
            note: pending(Some("skill"), SKILLS, ProgressListUnknown::FactMissing),
        },
        Case {
            name: "a skills list without the key",
            given: skill_2,
            lists: vec![roster_a(), skills(&[("unit_b", FactScalar::Integer(1))])],
            note: reading_missing.clone(),
        },
        Case {
            name: "a row without the reading column",
            given: skill_2,
            lists: vec![
                roster_a(),
                list(
                    SKILLS,
                    vec![BTreeMap::from([("unit".to_owned(), text("unit_a"))])],
                ),
            ],
            note: reading_missing,
        },
        Case {
            name: "a reading of another kind",
            given: skill_2,
            lists: vec![roster_a(), skills_a(text("x"))],
            note: position_unknown.clone(),
        },
        Case {
            name: "a reading equal to no position",
            given: skill_2,
            lists: vec![roster_a(), skills_a(FactScalar::Integer(9))],
            note: position_unknown,
        },
        Case {
            name: "an entry the root does not apply",
            given: ("other", "unit_a", r#"{"skill":[2]}"#),
            lists: vec![roster_a(), skills_a(FactScalar::Integer(1))],
            note: not_applied("other", "unit_a", GoalProblem::NameUnknown),
        },
        Case {
            name: "a key the table does not hold",
            given: (ENTRY, "unit_z", r#"{"skill":[2]}"#),
            lists: vec![roster_a(), skills_a(FactScalar::Integer(1))],
            note: not_applied(ENTRY, "unit_z", GoalProblem::NotAnOption),
        },
        Case {
            name: "a position the ladder no longer holds",
            given: (ENTRY, "unit_a", r#"{"skill":[7]}"#),
            lists: vec![roster_a(), skills_a(FactScalar::Integer(1))],
            note: not_applied(ENTRY, "unit_a", GoalProblem::OutOfRange),
        },
    ];
    for Case {
        name,
        given,
        lists,
        note,
    } in cases
    {
        let needs = walked(&[given], lists, vec![("pool.book", known(4))]);
        assert!(needs.pools().is_empty(), "{name}: {:?}", needs.pools());
        assert_eq!(needs.notes(), [note], "{name}");
    }

    // A holding not known: the item's whole need on its pool, nothing below it.
    let needs = walked(
        &[skill_2],
        vec![roster_a(), skills_a(FactScalar::Integer(1))],
        vec![(
            "pool.book",
            NeedHolding::Unknown(Some(ResourceTargetPendingReason::Missing)),
        )],
    );
    assert_eq!(
        needs.pools(),
        &pools(&[("pool.book", 5, PoolNeedKind::Unsplit)])
    );
    assert_eq!(
        needs.notes(),
        [
            RecordNeedsNote::Condition {
                record: record("unit_a"),
                column: "skill".to_owned(),
                condition: "side_story".to_owned(),
            },
            RecordNeedsNote::HoldingUnknown {
                item: "book".to_owned(),
                pool_id: "pool.book".to_owned(),
                reason: Some(ResourceTargetPendingReason::Missing),
            },
        ]
    );

    // The lower bound: with skill not known only grade is walked, and no pool needs more than it
    // needs with skill known.
    let both = (ENTRY, "unit_b", r#"{"grade":[2],"skill":[3]}"#);
    let full = walked(
        &[both],
        vec![
            roster(&[("unit_b", 0)]),
            skills(&[("unit_b", FactScalar::Integer(1))]),
        ],
        vec![("pool.book", known(4))],
    );
    let partial = walked(
        &[both],
        vec![roster(&[("unit_b", 0)])],
        vec![("pool.book", known(4))],
    );
    assert_eq!(
        partial.pools(),
        &pools(&[("pool.gold", 300, PoolNeedKind::Lands)])
    );
    for (pool, need) in partial.pools() {
        assert!(
            full.pools()
                .get(pool)
                .is_some_and(|known_need| known_need.need >= need.need),
            "{pool}: {:?} against {:?}",
            partial.pools(),
            full.pools()
        );
    }
    assert!(full.pools().len() > partial.pools().len());
}

// Test 4, two instances of one root (ruling 1). The same record on two instances whose rosters
// stand at grade 0 and grade 1 needs 300 and 200 `gold`; each set is its own, of another identity.
#[test]
fn rr3_p4_two_instances_of_one_root_compute_apart() {
    let walk_root = root(tables(&walk_tables()), SHA);
    let of = |alias: &str, grade: i64| {
        let sources = Sources {
            lists: BTreeMap::from([(ROSTER.to_owned(), roster(&[("unit_a", grade)]))]),
            holdings: BTreeMap::new(),
        };
        instance_needs(
            &stored(alias, &[(ENTRY, "unit_a", r#"{"grade":[2]}"#)]),
            NOW,
            Some(&walk_root),
            &sources,
        )
        .expect("an instance with records has needs")
    };
    let first = of(ALIAS, 0);
    let second = of(POLICY_INSTANCE_ALIAS_B, 1);
    assert_eq!(
        first.pools(),
        &pools(&[("pool.gold", 300, PoolNeedKind::Lands)])
    );
    assert_eq!(
        second.pools(),
        &pools(&[("pool.gold", 200, PoolNeedKind::Lands)])
    );
    assert_ne!(first.identity(), second.identity());
}

// ---- The host

/// The pool the host fixture's need lands on, read from rows of [`LIST_KEY`].
const LISTED: &str = "pool.listed";
const LIST_KEY: &str = "inventory.list.example_kind_a";
const TASK: &str = "fixture.observe";
/// The instance's game, by which the fixture's roots are found.
const GAME: &str = "fixture-game-a";

/// #455 P2's fixture catalog declared V2: its pool [`LISTED`] read from rows `item_a` × 200 and
/// `item_b` × 400 of [`LIST_KEY`], valued (step 1, base 1000, gap weight 100); its task due while
/// the pool holds at least 1000 and producing 1 of it; its profiles without goals.
fn listed_sources() -> CatalogSources {
    let mut sources = policy_sources(1);
    for source in [
        &mut sources.tasks,
        &mut sources.pools,
        &mut sources.activity,
        &mut sources.timeline,
    ] {
        let mut document: Value =
            serde_json::from_slice(&source.bytes).expect("the fixture's JSON");
        document["schema_version"] = json!(actingcommand_policy::SCHEDULING_SCHEMA_VERSION_V2);
        if let Some(events) = document.get_mut("events").and_then(Value::as_array_mut) {
            for event in events {
                event["validity"] = json!({"from_unix_ms": 0, "until_unix_ms": null});
            }
        }
        if document.get("tasks").is_some() {
            document["tasks"][0]["trigger"] = json!({
                "kind": "resource_projection",
                "pool_id": LISTED,
                "comparison": "greater_than_or_equal",
                "value": 1_000
            });
            document["tasks"][0]["produces"] = json!([{
                "pool_id": LISTED,
                "direction": "produce",
                "amount": 1,
                "observation_source": "scan_verified",
                "confidence_milli": 1000
            }]);
        }
        if document.get("pools").is_some() {
            document["pools"] = json!([{
                "id": LISTED,
                "scope": {"kind": "instance", "instance_id": ALIAS},
                "capacity": 10_000,
                "projection": {"kind": "none"},
                "observation": {
                    "kind": "list_rows",
                    "fact_key": LIST_KEY,
                    "key_column": "id",
                    "value_column": "count",
                    "rows": [{"key": "item_a", "factor": 200}, {"key": "item_b", "factor": 400}]
                },
                "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
                "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
                "valuation": {
                    "name": "Fixture",
                    "unit": "unit",
                    "scale": 1,
                    "base_weight_milli": 1000,
                    "gap": {"rule": "shortfall_linear", "weight_milli": 100}
                }
            }]);
        }
        if let Some(profiles) = document.get_mut("profiles").and_then(Value::as_array_mut) {
            for profile in profiles {
                profile["goals"] = json!([]);
            }
        }
        source.bytes = serde_json::to_vec_pretty(&document).expect("the fixture's bytes");
    }
    sources
}

/// The tables of the host fixture: `coin`, held in [`LISTED`]; a class's grade ladder `[0] [1]
/// [2]` of 1000 `coin`, then `second` `coin`, read from the roster.
fn coin_tables(second: u64) -> StandardPackGoalTables {
    tables(&json!({
        "schema_version": "actingcommand.goal-tables.v1",
        "game": TABLES_GAME,
        "items": {"coin": {"pool": LISTED}},
        "tables": [{
            "name": TABLE,
            "progress": [{"fact_key": ROSTER, "key_column": "unit", "held_by": true, "columns": ["grade"]}],
            "columns": [{"name": "grade", "read": ["grade"], "per": "class"}],
            "classes": {"all": {"grade": {
                "positions": [[0], [1], [2]],
                "steps": [{"coin": 1_000}, {"coin": second}]
            }}},
            "keys": {"unit_a": {"class": "all"}}
        }]
    }))
}

/// What the host's needs read of the instance's game: [`coin_tables`] of `second`, of `sha`.
fn coin_roots(second: u64, sha: &str) -> RecordRoots {
    RecordRoots::from([(GAME.to_owned(), root(coin_tables(second), sha))])
}

/// A host of the policy instance on the fixture's manual clock, with [`listed_sources`] active
/// and the needs reading [`coin_roots`] of 2000.
fn start(root_dir: &TempDir) -> RuntimeHost {
    let sources = listed_sources();
    if let Err(failure) = actingcommand_policy::compile_catalog(&sources) {
        panic!(
            "the fixture's catalog compiles: {:?}",
            failure.diagnostics()
        );
    }
    let host = RuntimeHost::start(
        config(root_dir)
            .with_runtime_clock(Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0))),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("the runtime host");
    host.activate_policy_catalog(&sources)
        .expect("the catalog, configured directly");
    host.set_goal_record_roots_for_test(Some(coin_roots(2_000, SHA)))
        .expect("the needs' roots");
    host
}

// The fixture's time. Named `t0`, not `now`: the gate maps' determinism guard takes a call named
// `now` for a read of the wall clock (the first build failed on it).
fn t0() -> EvaluationTime {
    EvaluationTime {
        unix_ms: POLICY_NOW_UNIX_MS,
        monotonic_ms: POLICY_NOW_UNIX_MS,
    }
}

/// One evaluated cycle at the fixture's time over the fixture's facts, no pool snapshot
/// configured.
fn evaluate(host: &RuntimeHost) -> PolicyEvaluation {
    let mut resources = policy_resources();
    resources.pools.clear();
    host.evaluate_policy_cycle_with_test_inputs(
        &policy_facts(),
        &resources,
        t0(),
        7,
        PolicyTrigger::Recovery,
    )
    .expect("a policy cycle")
    .evaluation
    .expect("an evaluated cycle")
}

fn decision(evaluation: &PolicyEvaluation) -> &TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| decision.task_id == TASK && decision.instance_id.as_deref() == Some(ALIAS))
        .unwrap_or_else(|| panic!("{TASK}'s decision: {evaluation:#?}"))
}

/// The evaluation's intent of [`TASK`] and its reason chain.
fn intent_and_chain(evaluation: &PolicyEvaluation) -> (DispatchIntent, DecisionReasonChain) {
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == TASK && intent.instance_id == ALIAS)
        .unwrap_or_else(|| panic!("the task is due: {evaluation:#?}"))
        .clone();
    let chain = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("the intent's reason chain")
        .clone();
    (intent, chain)
}

/// The list of [`LIST_KEY`]: `item_a` 3 and `item_b` 2, so the pool reads 1400.
fn listed() -> FactRecord {
    let mut fact = list(
        LIST_KEY,
        vec![
            BTreeMap::from([
                ("id".to_owned(), text("item_a")),
                ("count".to_owned(), FactScalar::Integer(3)),
            ]),
            BTreeMap::from([
                ("id".to_owned(), text("item_b")),
                ("count".to_owned(), FactScalar::Integer(2)),
            ]),
        ],
    );
    fact.source_snapshot_id = "snapshot:rr3-listed".to_owned();
    fact
}

/// The roster with `unit_a` at `grade`, under the snapshot `snapshot`.
fn roster_at(grade: i64, snapshot: &str) -> FactRecord {
    let mut fact = roster(&[("unit_a", grade)]);
    fact.source_snapshot_id = snapshot.to_owned();
    fact
}

/// The instance's `session.goal_records` record of one record of `unit_a` to `to`, as the goal
/// edit stores it (no expiry), its identity the hash of `to`.
fn records(to: &str, snapshot: &str) -> FactRecord {
    let mut fact = stored_fact(
        own_scope(),
        GOAL_RECORDS_FACT_KEY,
        ContractFactValue::RecordList(vec![header(ALIAS), record_row(ENTRY, "unit_a", to)]),
        snapshot,
        Vec::new(),
    );
    fact.expires_at_unix_ms = None;
    fact.ttl_policy = None;
    fact.confidence_milli = 1_000;
    fact.resource_bundle_hash = format!("{:x}", Sha256::digest(to.as_bytes()));
    fact
}

/// Publishes the pool's list (1400), the roster with `unit_a` at grade 0 and the records raising
/// it to grade 2.
fn publish_inputs(host: &RuntimeHost) {
    host.publish_list_reading_for_test(listed())
        .expect("the pool's list");
    host.publish_list_reading_for_test(roster_at(0, "snapshot:rr3-roster-a"))
        .expect("the roster");
    host.publish_goal_records_for_test(records(r#"{"grade":[2]}"#, "snapshot:rr3-records-a"))
        .expect("the records");
}

// Test 5, the host computes and the evaluator pushes (rulings 1, 5, 6 and 8). With the records
// raising `unit_a` from grade 0 to 2, the needs computed from the store, the roots and the pool's
// reading are 3000 `coin`, landing on the pool, fresh until the roster's expiry; the evaluation's
// producer of the pool carries `goal_records_need`: 3000 needed, no stock, 1400 held, all of it to
// the characters, the gap 1600.
#[test]
fn rr3_h1_the_host_computes_the_needs_and_the_producer_of_the_pool_is_pushed() {
    let root_dir = TempDir::new().expect("tempdir");
    let host = start(&root_dir);
    evaluate(&host);
    assert!(
        host.record_needs_for_test(t0())
            .expect("the needs")
            .is_empty(),
        "no records, no needs"
    );
    publish_inputs(&host);
    let needs = host.record_needs_for_test(t0()).expect("the needs");
    let computed = needs
        .get(ALIAS)
        .unwrap_or_else(|| panic!("the instance's needs: {needs:?}"));
    assert_eq!(
        computed.pools(),
        &pools(&[(LISTED, 3_000, PoolNeedKind::Lands)])
    );
    assert!(computed.notes().is_empty(), "{:?}", computed.notes());
    assert_eq!(computed.tables_sha256(), Some(SHA));
    assert_eq!(computed.fresh_until_unix_ms(), Some(READ_AT + FRESH_MS));

    let evaluation = evaluate(&host);
    let task = decision(&evaluation);
    let need = task
        .reasons
        .iter()
        .find(|reason| reason.outcome.code().as_str() == "goal_records_need")
        .unwrap_or_else(|| panic!("a goal_records_need reason: {task:#?}"));
    assert_eq!(
        need.outcome.value("pool_id").and_then(Value::as_str),
        Some(LISTED)
    );
    assert_eq!(
        need.detail,
        format!("needed=3000 stock=0 held=1400 to_characters=1400 gap=1600 tables={SHA}")
    );
    intent_and_chain(&evaluation);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

/// What changes under an intent in test 6.
#[derive(Debug, Clone, Copy)]
enum Change {
    /// The roster is read again, `unit_a` at grade 1.
    List,
    /// The records are set again, `unit_a` to grade 1.
    Record,
    /// The root's tables change (another sha256, another second step).
    Tables,
}

// Test 6, the identities (ruling 6). An intent evaluated on the needs of test 5 is refused
// `policy_facts_stale` once the roster is read again, the records are set again or the root's
// tables change: the needs are in the instance's view, and the needs' identity moved (the roster
// and the tables are in no snapshot identity).
#[test]
fn rr3_h2_an_intent_is_stale_once_a_progress_list_the_records_or_the_tables_change() {
    for change in [Change::List, Change::Record, Change::Tables] {
        let root_dir = TempDir::new().expect("tempdir");
        let host = start(&root_dir);
        evaluate(&host);
        publish_inputs(&host);
        let evaluation = evaluate(&host);
        let (intent, reasons) = intent_and_chain(&evaluation);
        record_policy_approval(&host, &intent);
        let before = host.record_needs_for_test(t0()).expect("the needs");
        let changed = match change {
            Change::List => host
                .publish_list_reading_for_test(roster_at(1, "snapshot:rr3-roster-b"))
                .map(|_| ()),
            Change::Record => host.publish_goal_records_for_test(records(
                r#"{"grade":[1]}"#,
                "snapshot:rr3-records-b",
            )),
            Change::Tables => {
                host.set_goal_record_roots_for_test(Some(coin_roots(500, "fixture-tables-b")))
            }
        };
        changed.unwrap_or_else(|error| panic!("{change:?}: {error:?}"));
        let after = host.record_needs_for_test(t0()).expect("the needs");
        assert_ne!(
            after.get(ALIAS).map(InstanceRecordNeeds::identity),
            before.get(ALIAS).map(InstanceRecordNeeds::identity),
            "{change:?}"
        );
        assert_eq!(
            host.admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
                .unwrap_err()
                .code(),
            "policy_facts_stale",
            "{change:?}"
        );
        assert!(host.fatal_error().expect("runtime health").is_none());
        host.close().expect("close the host");
    }
}
