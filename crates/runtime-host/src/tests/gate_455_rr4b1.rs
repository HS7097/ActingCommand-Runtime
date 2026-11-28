// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #455 RR4b-1 (0.12.104; register §1.557; contracts/runtime-interface.md, "Goal request
// kinds", "The goal page in pages"; contracts/standard-pack-goals.md, "What a client sees";
// contracts/standard-pack-goal-tables.md, "Who reads it"; coordinator work order
// rd-1123/brief-455-rr4b1.md with its Addendum 1): the goal page's records with their positions, a
// table's options and one ladder's positions, read in pages by `GoalPageView`, which no client of
// `runtime-client` 14 sends; `GoalView` is not touched (its gates stay as they are).
//
// The pure tests run `crate::goal_needs::walk_records`, the one walk the needs' computation and the
// read call, and `crate::goal_page::read_part`, on a made-up root: a `goals.json` of a heading and
// three records entries (two on the table `units`, one on a table the tables file does not have),
// and a `goal-tables.json` whose table reads a class ladder from a roster (its `held_by` list) and
// a key's own ladders from a second list, with names, a prerequisite with `from` and a condition.
// The host tests run the host in composer mode over one game root on disk, as `gate_455_rr2` does,
// with a manual Runtime clock and fake devices. No package runs; no clock is read and nothing
// sleeps. Every name and amount is made up.

use super::gate_442_rem1::write_package;
use super::gate_rl1b::{
    GAME_A, POOL_A, SERVER_A, bundle, pool, profiles, resources, rule, write_json,
};
use super::*;
use crate::goal_needs::{KeyHeld, NeedsSources, RecordsRoot, instance_needs, walk_records};
use crate::goal_page::{PageFailure, PageRead, PageState, entries_digest, page_head, read_part};
use crate::goal_records::{GOAL_RECORDS_SCHEMA_VERSION, StoredGoalRecords};
use actingcommand_contract::codes::RuntimeContractCode;
use actingcommand_contract::{
    DeclaredInitiator, FactScalar, GoalClass, GoalColumn, GoalColumnRead, GoalDeclarationRead,
    GoalDeclarationState, GoalEdit, GoalEdited, GoalKey, GoalKeyColumn, GoalLadder, GoalLadderRef,
    GoalPageCursor, GoalPageCut, GoalPagePart, GoalPageRecordsState, GoalPageTables, GoalPageView,
    GoalPositionSegment, GoalProblem, GoalProgressList, GoalTable, GoalTableItem, GoalTableNames,
    GoalTableNaming, GoalTablesRead, GoalText, GoalUse, GoalView, Initiator,
    MAX_GOAL_PAGE_VIEW_BYTES, MAX_GOAL_PAGE_VIEW_ROW_BYTES, RuntimeChannel, StandardPackGoalTables,
    StandardPackGoals, parse_standard_pack_goal_tables, parse_standard_pack_goals,
};
use actingcommand_policy::{
    NeedHolding, PoolNeed, PoolNeedKind, ProgressListUnknown, RecordNeedsNote, RecordRef,
};
use serde_json::{Value, json};

// ---- The pure part ----

const ALIAS: &str = POLICY_INSTANCE_ALIAS;
const NOW: u64 = POLICY_NOW_UNIX_MS;
/// When the fixture's lists are read: ten seconds before the fixture's time.
const READ_AT: u64 = POLICY_NOW_UNIX_MS - 10_000;
/// How long a list read stays fresh.
const FRESH_MS: u64 = 60_000;
/// The table's `held_by` list, which gives `grade`.
const ROSTER: &str = "inventory.list.fixture_roster";
/// The table's second list, which gives `skill` and `mastery_1`.
const SKILLS: &str = "inventory.list.fixture_skills";
/// The two records entries on [`TABLE`].
const ENTRY: &str = "raise";
const SECOND: &str = "raise_too";
const TABLE: &str = "units";
/// The game the fixture's files are written for.
const GAME: &str = "neutral";

/// The sha256 the workbench would keep for the fixture's tables (made up, of its form).
fn sha() -> String {
    format!("sha256:{}", "a".repeat(64))
}

/// What the walk reads besides the records and the root: lists by key (fresh until their expiry
/// at the fixture's time); no holding is known.
struct Sources {
    lists: BTreeMap<String, FactRecord>,
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

    fn holding(&self, _pool_id: &str) -> NeedHolding {
        NeedHolding::Unknown(None)
    }
}

fn sources(lists: Vec<FactRecord>) -> Sources {
    Sources {
        lists: lists
            .into_iter()
            .map(|read| (read.key.clone(), read))
            .collect(),
    }
}

fn text(value: &str) -> FactScalar {
    FactScalar::String(value.to_owned())
}

/// The list `fact_key` of `rows` of the fixture's instance, read at [`READ_AT`], of the expiry
/// `expires_at_unix_ms`, at confidence 900 (the shared fact's lifetime policy, 1 s to 2 min, holds
/// a list fresh for [`FRESH_MS`]).
fn list_at(
    fact_key: &str,
    rows: Vec<BTreeMap<String, FactScalar>>,
    expires_at_unix_ms: u64,
) -> FactRecord {
    let mut fact = stored_fact(
        FactScope::Instance {
            instance_id: ALIAS.to_owned(),
        },
        fact_key,
        ContractFactValue::RecordList(rows),
        &format!("snapshot:rr4b1:{fact_key}"),
        Vec::new(),
    );
    fact.observed_at_unix_ms = READ_AT;
    fact.expires_at_unix_ms = Some(expires_at_unix_ms);
    fact.confidence_milli = 900;
    fact
}

/// The list `fact_key` of `rows`, fresh for [`FRESH_MS`].
fn list(fact_key: &str, rows: Vec<BTreeMap<String, FactScalar>>) -> FactRecord {
    list_at(fact_key, rows, READ_AT + FRESH_MS)
}

/// A row `{unit, <column>: <value>...}` of a list.
fn unit_row(unit: &str, columns: &[(&str, i64)]) -> BTreeMap<String, FactScalar> {
    let mut row = BTreeMap::from([("unit".to_owned(), text(unit))]);
    for (column, value) in columns {
        row.insert((*column).to_owned(), FactScalar::Integer(*value));
    }
    row
}

/// The roster of `units`, each with its grade.
fn roster(units: &[(&str, i64)]) -> FactRecord {
    list(
        ROSTER,
        units
            .iter()
            .map(|(unit, grade)| unit_row(unit, &[("grade", *grade)]))
            .collect(),
    )
}

/// The skills list of `units`, each with its skill and its first mastery.
fn skills(units: &[(&str, i64, i64)]) -> FactRecord {
    list(
        SKILLS,
        units
            .iter()
            .map(|(unit, skill, mastery)| {
                unit_row(unit, &[("skill", *skill), ("mastery_1", *mastery)])
            })
            .collect(),
    )
}

/// The header row of the fixture's instance's `session.goal_records` record.
fn header() -> BTreeMap<String, FactScalar> {
    BTreeMap::from([
        ("row".to_owned(), text("header")),
        (
            "schema_version".to_owned(),
            text(GOAL_RECORDS_SCHEMA_VERSION),
        ),
        ("instance".to_owned(), text(ALIAS)),
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

/// The instance's stored records `given` (entry, key, to), read by the goal records' own reader.
fn stored(given: &[(&str, &str, &str)]) -> StoredGoalRecords {
    let mut rows = vec![header()];
    rows.extend(
        given
            .iter()
            .map(|(entry, key, to)| record_row(entry, key, to)),
    );
    let read = StoredGoalRecords::read(Some(rows.as_slice()), ALIAS);
    assert!(
        matches!(&read, StoredGoalRecords::Read(taken) if taken.len() == given.len()),
        "the fixture's records read: {read:?}"
    );
    read
}

/// The fixture's `goal-tables.json`: the table `units` with a class column `grade` read from the
/// roster (ladder `[0] [1] [2]`, 100 and 200 `gold`), each key's own `skill` read from the skills
/// list (`[1] [2] [3]`, 5 `book`, then 7 `book` and 3 of its own shard), whose steps that arrive at
/// `[3]` or beyond ask grade `[2]` and every step the condition `side_story`, and a column `mastery`
/// of one sub `1` read from the skills list, which only `unit_a` has (`[0] [1]`, 1 `gold`); the
/// names of `unit_a`, of `grade` and its positions (in two languages), of the sub, of the class and
/// of the condition, and how `mastery`'s positions are said.
fn units_document() -> Value {
    let skill =
        json!({"positions": [[1], [2], [3]], "steps": [{"book": 5}, {"book": 7, "@shard": 3}]});
    json!({
        "schema_version": "actingcommand.goal-tables.v1",
        "game": GAME,
        "items": {
            "gold": {"pool": "pool.gold"},
            "book": {"pool": "pool.book"},
            "shard_a": {"pool": "pool.shard_a"},
            "shard_b": {"pool": "pool.shard_b"}
        },
        "tables": [{
            "name": TABLE,
            "progress": [
                {"fact_key": ROSTER, "key_column": "unit", "held_by": true, "columns": ["grade"]},
                {"fact_key": SKILLS, "key_column": "unit", "columns": ["skill", "mastery_1"]}
            ],
            "columns": [
                {"name": "grade", "read": ["grade"], "per": "class"},
                {"name": "skill", "read": ["skill"], "per": "key", "requires": [
                    {"column": "grade", "at_least": [2], "from": [3]},
                    {"condition": "side_story"}
                ]},
                {"name": "mastery", "per": "sub", "subs": {"1": ["mastery_1"]}}
            ],
            "classes": {"all": {"grade": {"positions": [[0], [1], [2]], "steps": [{"gold": 100}, {"gold": 200}]}}},
            "keys": {
                "unit_a": {"class": "all", "own": {"shard": "shard_a"}, "skill": skill,
                           "mastery": {"1": {"positions": [[0], [1]], "steps": [{"gold": 1}]}}},
                "unit_b": {"class": "all", "own": {"shard": "shard_b"}, "skill": skill},
                "unit_c": {"class": "all", "own": {"shard": "shard_a"}, "skill": skill}
            }
        }],
        "names": {"tables": {TABLE: {
            "keys": {"unit_a": {"en": "First unit"}},
            "columns": {"grade": {"en": "Grade"}},
            "subs": {"mastery": {"1": {"en": "First mastery"}}},
            "classes": {"all": {"en": "Everyone"}},
            "conditions": {"side_story": {"en": "The side story"}},
            "positions": {
                "grade": [{"prefix": [], "text": {"en": "Grade {0}", "fr": "Rang {0}"}}],
                "mastery": [{"prefix": [], "text": {"en": "Mastery {0}"}}]
            }
        }}}
    })
}

/// The fixture's `goals.json` (version 3): a heading, the records entries [`ENTRY`] and [`SECOND`]
/// on [`TABLE`] (at most `max_records` each), and `spare` on a table the tables file does not have.
fn goals_document(max_records: u64) -> Value {
    json!({"schema_version": "actingcommand.goals.v3", "game": GAME, "entries": [
        {"name": "intro", "type": "heading", "title": {"en": "Raising"}},
        {"name": ENTRY, "type": "records", "table": TABLE, "max_records": max_records,
         "title": {"en": "Whom to raise"}},
        {"name": SECOND, "type": "records", "table": TABLE, "max_records": max_records,
         "title": {"en": "More to raise"}},
        {"name": "spare", "type": "records", "table": "missing", "max_records": 1,
         "title": {"en": "Spare"}}
    ]})
}

fn goals_of(document: &Value) -> StandardPackGoals {
    parse_standard_pack_goals(document.to_string().as_bytes(), GAME)
        .unwrap_or_else(|issues| panic!("the fixture's goals read: {issues:?}"))
}

fn tables_of(document: &Value) -> StandardPackGoalTables {
    parse_standard_pack_goal_tables(document.to_string().as_bytes(), GAME)
        .unwrap_or_else(|issues| panic!("the fixture's tables read: {issues:?}"))
}

/// The root as the workbench keeps it: its `goals.json`, its tables and what the needs read of
/// them (each records entry with its table, the tables with their sha256).
struct Fixture {
    goals: StandardPackGoals,
    tables: StandardPackGoalTables,
    root: RecordsRoot,
}

impl Fixture {
    fn of(goals: StandardPackGoals, tables: StandardPackGoalTables) -> Self {
        let entries = goals
            .entries
            .iter()
            .filter_map(|goal| match &goal.kind {
                actingcommand_contract::DeclaredGoalKind::Records { table, .. } => {
                    Some((goal.name.clone(), table.clone()))
                }
                _ => None,
            })
            .collect();
        let root = RecordsRoot {
            entries,
            tables: Some((Arc::new(tables.clone()), sha())),
        };
        Self {
            goals,
            tables,
            root,
        }
    }

    fn units() -> Self {
        Self::of(goals_of(&goals_document(8)), tables_of(&units_document()))
    }

    /// What a cursor is compared with on this root, the stored records' version
    /// `records_version`.
    fn seq(&self, records_version: u64) -> PageState {
        PageState {
            records_version,
            tables_sha256: Some(sha()),
            entries_sha256: entries_digest(Some(&self.goals)),
        }
    }

    /// One page of `part` on this root, against `now`.
    fn read(
        &self,
        part: &GoalPagePart,
        stored: &StoredGoalRecords,
        sources: &Sources,
        cursor: Option<&GoalPageCursor>,
        limit: Option<u32>,
        now: PageState,
    ) -> Result<GoalPageView, PageFailure> {
        let read = PageRead {
            declaration: Some(&self.goals),
            tables: Some(&self.tables),
            root: Some(&self.root),
            stored,
            now,
        };
        read_part(head(), &read, sources, part, cursor, limit)
    }
}

/// The head of a page of the fixture's instance: version 7, both files accepted.
fn head() -> GoalPageView {
    page_head(
        ALIAS,
        GAME,
        7,
        GoalDeclarationState {
            state: GoalDeclarationRead::Accepted,
            sha256: None,
            standard_pack_problem: None,
            json_pointer: None,
            path: None,
            io_kind: None,
        },
        GoalPageTables {
            state: GoalTablesRead::Accepted,
            sha256: Some(sha()),
            standard_pack_problem: None,
            json_pointer: None,
        },
        NOW,
    )
}

/// The rows of `page` as a client reads them.
fn rows_of(page: &GoalPageView) -> Vec<Value> {
    page.rows
        .iter()
        .map(|row| serde_json::to_value(row).expect("a row as JSON"))
        .collect()
}

/// Each row of `page` by its word and what it names: `entry:<name>`, `record:<entry>/<key>`,
/// `column:<column>`, `class:<class>`, `key:<key>`, `position:<index>`.
fn shape(page: &GoalPageView) -> Vec<String> {
    rows_of(page)
        .iter()
        .map(|row| {
            let word = row["row"].as_str().expect("a row word");
            let named = match word {
                "entry" => row["name"].as_str().map(str::to_owned),
                "record" => Some(format!(
                    "{}/{}",
                    row["entry"].as_str().unwrap_or_default(),
                    row["key"].as_str().unwrap_or_default()
                )),
                "column" => row["column"].as_str().map(str::to_owned),
                "class" => row["class"].as_str().map(str::to_owned),
                "key" => row["key"].as_str().map(str::to_owned),
                _ => Some(row["index"].to_string()),
            };
            format!("{word}:{}", named.unwrap_or_default())
        })
        .collect()
}

fn record(entry: &str, key: &str) -> RecordRef {
    RecordRef {
        entry: entry.to_owned(),
        key: key.to_owned(),
    }
}

// Ruling 4 and the study's 2.9: the needs' computation and the read take their judgement from one
// walk. Five stored records: `unit_a` to skill `[2]`, `unit_b` to grade `[0]` and skill `[3]`,
// `unit_c` to grade `[1]` (behind its progress), `unit_a` again under the second entry
// (`key_repeated`), and one under a name the root does not declare (`name_unknown`). The needs are
// RR3's (17 `book`, 300 `gold`, 3 of `unit_b`'s shard, each landing on its pool), their notes the
// walk's own, in its order, and their freshness the lists'. The walk answers, record by record, the
// same judgement: `unit_b`'s grade target `[0]` raised to `[2]` by the prerequisite its skill step
// to `[3]` asks, from its current `[0]`; its skill from `[1]` to `[3]` with the condition. The
// page's rows, in order: each records entry of the file (whether it applies, the entry before it)
// with its records, the entry whose table the file does not have, then the undeclared name with its
// record; `unit_b`'s row says each lane's target, raised position and current position in both of
// the pack's languages, its key held by the roster, nothing it lacks.
#[test]
fn rr4b1_p1_the_needs_and_the_page_take_their_judgement_from_one_walk() {
    let fixture = Fixture::units();
    let given = [
        (ENTRY, "unit_a", r#"{"skill":[2]}"#),
        (ENTRY, "unit_b", r#"{"grade":[0],"skill":[3]}"#),
        (ENTRY, "unit_c", r#"{"grade":[1]}"#),
        (SECOND, "unit_a", r#"{"grade":[2]}"#),
        ("other", "unit_a", r#"{"grade":[1]}"#),
    ];
    let records = stored(&given);
    let lists = sources(vec![
        roster(&[("unit_a", 0), ("unit_b", 0), ("unit_c", 2)]),
        skills(&[("unit_a", 1, 0), ("unit_b", 1, 0), ("unit_c", 1, 0)]),
    ]);
    let StoredGoalRecords::Read(rows) = &records else {
        panic!("the fixture's records read");
    };
    let walk = walk_records(rows, Some(&fixture.root), &lists);
    let needs = instance_needs(&records, NOW, Some(&fixture.root), &lists)
        .expect("an instance with records has needs");
    let lands = |need| PoolNeed {
        need,
        kind: PoolNeedKind::Lands,
    };
    assert_eq!(
        needs.pools(),
        &BTreeMap::from([
            ("pool.book".to_owned(), lands(17)),
            ("pool.gold".to_owned(), lands(300)),
            ("pool.shard_b".to_owned(), lands(3)),
        ])
    );
    let condition = |key: &str| RecordNeedsNote::Condition {
        record: record(ENTRY, key),
        column: "skill".to_owned(),
        condition: "side_story".to_owned(),
    };
    let expected_notes = vec![
        condition("unit_a"),
        condition("unit_b"),
        RecordNeedsNote::NotApplied {
            record: Some(record(SECOND, "unit_a")),
            problem: GoalProblem::KeyRepeated,
        },
        RecordNeedsNote::NotApplied {
            record: Some(record("other", "unit_a")),
            problem: GoalProblem::NameUnknown,
        },
    ];
    assert_eq!(needs.notes(), expected_notes.as_slice());
    assert_eq!(walk.notes, expected_notes);
    assert_eq!(needs.fresh_until_unix_ms(), walk.fresh_until_unix_ms);
    assert_eq!(walk.fresh_until_unix_ms, Some(READ_AT + FRESH_MS));

    assert_eq!(walk.records.len(), given.len());
    assert_eq!(walk.records[3], Err(GoalProblem::KeyRepeated));
    assert_eq!(walk.records[4], Err(GoalProblem::NameUnknown));
    let unit_b = walk.records[1].as_ref().expect("unit_b is applied");
    assert_eq!(
        unit_b.held,
        KeyHeld::Held {
            fact_key: ROSTER.to_owned()
        }
    );
    let grade = &unit_b.lanes[&("grade".to_owned(), None)];
    assert_eq!(
        (grade.target, grade.walked_to, grade.current.clone()),
        (Some(0), 2, Some(Ok(0)))
    );
    let skill = &unit_b.lanes[&("skill".to_owned(), None)];
    assert_eq!(
        (skill.target, skill.walked_to, skill.current.clone()),
        (Some(2), 2, Some(Ok(0)))
    );
    assert_eq!(skill.conditions, vec!["side_story".to_owned()]);

    let page = fixture
        .read(
            &GoalPagePart::Records,
            &records,
            &lists,
            None,
            None,
            fixture.seq(3),
        )
        .expect("the records' page");
    assert_eq!(
        shape(&page),
        [
            "entry:raise",
            "record:raise/unit_a",
            "record:raise/unit_b",
            "record:raise/unit_c",
            "entry:raise_too",
            "record:raise_too/unit_a",
            "entry:spare",
            "entry:other",
            "record:other/unit_a",
        ]
    );
    assert_eq!(page.rows_total, 9);
    assert_eq!((page.next.as_ref(), page.cut), (None, None));
    assert_eq!(page.records_state, Some(GoalPageRecordsState::Read));
    assert_eq!(page.fresh_until_unix_ms, Some(READ_AT + FRESH_MS));
    let rows = rows_of(&page);
    assert_eq!(
        (
            &rows[0]["declared"],
            &rows[0]["after_entry"],
            &rows[0]["applies"],
            &rows[0]["records_total"],
            &rows[0]["entry"]["table"],
        ),
        (
            &json!(true),
            &json!("intro"),
            &json!(true),
            &json!(3),
            &json!(TABLE)
        )
    );
    assert_eq!(
        rows[1],
        json!({"row": "record", "entry": ENTRY, "key": "unit_a",
               "key_name": {"en": "First unit"},
               "to": {"skill": [2]},
               "set": {"unix_ms": NOW, "channel": "cli"},
               "applied": true,
               "held": {"state": "held", "fact_key": ROSTER},
               "lanes": [{"column": "skill", "source": "own",
                          "target": {"index": 1, "position": [2]},
                          "current": {"index": 0, "position": [1]},
                          "conditions": [{"condition": "side_story",
                                          "name": {"en": "The side story"}}]}]})
    );
    let grade_said =
        |grade: i64| json!({"en": format!("Grade {grade}"), "fr": format!("Rang {grade}")});
    assert_eq!(
        rows[2],
        json!({"row": "record", "entry": ENTRY, "key": "unit_b",
               "to": {"grade": [0], "skill": [3]},
               "set": {"unix_ms": NOW, "channel": "cli"},
               "applied": true,
               "held": {"state": "held", "fact_key": ROSTER},
               "lanes": [
                   {"column": "grade", "column_name": {"en": "Grade"}, "source": "class",
                    "target": {"index": 0, "position": [0], "said": grade_said(0)},
                    "raised_to": {"index": 2, "position": [2], "said": grade_said(2)},
                    "current": {"index": 0, "position": [0], "said": grade_said(0)}},
                   {"column": "skill", "source": "own",
                    "target": {"index": 2, "position": [3]},
                    "current": {"index": 0, "position": [1]},
                    "conditions": [{"condition": "side_story",
                                    "name": {"en": "The side story"}}]}]})
    );
    assert_eq!(
        (
            &rows[3]["lanes"][0]["target"],
            &rows[3]["lanes"][0]["current"]
        ),
        (
            &json!({"index": 1, "position": [1], "said": grade_said(1)}),
            &json!({"index": 2, "position": [2], "said": grade_said(2)})
        ),
        "a target behind its progress is shown as it is"
    );
    assert_eq!(
        (&rows[4]["after_entry"], &rows[4]["records_total"]),
        (&json!(ENTRY), &json!(1))
    );
    assert_eq!(
        rows[5],
        json!({"row": "record", "entry": SECOND, "key": "unit_a",
               "key_name": {"en": "First unit"},
               "to": {"grade": [2]},
               "set": {"unix_ms": NOW, "channel": "cli"},
               "applied": false, "problem": "key_repeated", "lanes": []})
    );
    assert_eq!(
        (
            &rows[6]["applies"],
            &rows[6]["problem"],
            &rows[6]["records_total"]
        ),
        (&json!(false), &json!("name_unknown"), &json!(0))
    );
    assert_eq!(
        rows[7],
        json!({"row": "entry", "name": "other", "declared": false, "applies": false,
               "problem": "name_unknown", "records_total": 1})
    );
    assert_eq!(
        (&rows[8]["applied"], &rows[8]["problem"], &rows[8]["lanes"]),
        (&json!(false), &json!("name_unknown"), &json!([]))
    );
    for row in &rows {
        assert!(
            row.get("need").is_none() && row.get("needs").is_none(),
            "no amount of what a record lacks: {row}"
        );
    }
}

/// One case of test 2: the lists read, the held state and the lane `unit_a`'s skill record shows.
struct LaneCase {
    name: &'static str,
    lists: Vec<FactRecord>,
    held: Value,
    lane: Value,
}

// The lanes, case by case (ruling 2, the study's 3.1): a record of `unit_a` to skill `[2]`. Not
// known, or not held, the key's lanes carry their targets and no current position; a list that
// gives no reading, a list without the key's row, a row without the reading column and a reading
// that is no position each say why with the walk's own note code (and the list, the reading and the
// list's `unknown_reason` where they apply).
#[test]
fn rr4b1_p2_a_lane_not_known_says_why_with_the_walks_own_code() {
    let fixture = Fixture::units();
    let records = stored(&[(ENTRY, "unit_a", r#"{"skill":[2]}"#)]);
    let held = json!({"state": "held", "fact_key": ROSTER});
    let target_only = json!({"column": "skill", "source": "own",
                             "target": {"index": 1, "position": [2]}});
    let unknown = |unknown: Value| {
        json!({"column": "skill", "source": "own",
               "target": {"index": 1, "position": [2]}, "unknown": unknown})
    };
    let roster_a = || roster(&[("unit_a", 0)]);
    let cases = [
        LaneCase {
            name: "no roster",
            lists: vec![skills(&[("unit_a", 1, 0)])],
            held: json!({"state": "unknown", "fact_key": ROSTER, "unknown_reason": "fact_missing"}),
            lane: target_only.clone(),
        },
        LaneCase {
            name: "an expired roster",
            lists: vec![
                list_at(ROSTER, vec![unit_row("unit_a", &[("grade", 0)])], NOW - 1),
                skills(&[("unit_a", 1, 0)]),
            ],
            held: json!({"state": "unknown", "fact_key": ROSTER, "unknown_reason": "fact_expired"}),
            lane: target_only.clone(),
        },
        LaneCase {
            name: "a roster without the key",
            lists: vec![roster(&[("unit_b", 0)]), skills(&[("unit_a", 1, 0)])],
            held: json!({"state": "not_held", "fact_key": ROSTER}),
            lane: target_only.clone(),
        },
        LaneCase {
            name: "no skills list",
            lists: vec![roster_a()],
            held: held.clone(),
            lane: unknown(json!({
                "code": "goal_records_progress_unknown",
                "fact_key": SKILLS,
                "unknown_reason": "fact_missing"
            })),
        },
        LaneCase {
            name: "a skills list without the key",
            lists: vec![roster_a(), skills(&[("unit_b", 1, 0)])],
            held: held.clone(),
            lane: unknown(json!({"code": "goal_records_row_missing", "fact_key": SKILLS})),
        },
        LaneCase {
            name: "a row without the reading column",
            lists: vec![
                roster_a(),
                list(SKILLS, vec![unit_row("unit_a", &[("mastery_1", 0)])]),
            ],
            held: held.clone(),
            lane: unknown(json!({
                "code": "goal_records_reading_missing",
                "fact_key": SKILLS,
                "reading": "skill"
            })),
        },
        LaneCase {
            name: "a reading equal to no position",
            lists: vec![roster_a(), skills(&[("unit_a", 9, 0)])],
            held: held.clone(),
            lane: unknown(json!({"code": "goal_records_position_unknown"})),
        },
    ];
    for LaneCase {
        name,
        lists,
        held,
        lane,
    } in cases
    {
        let page = fixture
            .read(
                &GoalPagePart::Records,
                &records,
                &sources(lists),
                None,
                None,
                fixture.seq(1),
            )
            .unwrap_or_else(|failure| panic!("{name}: {failure:?}"));
        let rows = rows_of(&page);
        assert_eq!(rows[1]["held"], held, "{name}");
        assert_eq!(rows[1]["lanes"], json!([lane]), "{name}");
        assert_eq!(rows[1]["applied"], json!(true), "{name}");
    }
}

/// A text of 16 locales, each of about 100 bytes, `seed` first.
fn long_text(seed: &str) -> GoalText {
    (0..16)
        .map(|locale| {
            (
                format!("locale_{locale:02}"),
                format!("{seed} {}", "x".repeat(100)),
            )
        })
        .collect()
}

/// How every position is said: one segment of the empty prefix, 16 locales of about 113 bytes,
/// each with the position's first element.
fn long_said() -> Vec<GoalPositionSegment> {
    vec![GoalPositionSegment {
        prefix: Vec::new(),
        text: (0..16)
            .map(|locale| {
                (
                    format!("locale_{locale:02}"),
                    format!("{} {{0}}", "y".repeat(110)),
                )
            })
            .collect(),
    }]
}

/// A ladder of `positions` positions `[0]` to `[positions - 1]`, each step 1 `gold`.
fn counted_ladder(positions: i64) -> GoalLadder {
    GoalLadder {
        positions: (0..positions).map(|position| vec![position]).collect(),
        steps: (1..positions)
            .map(|_| vec![(GoalUse::Item("gold".to_owned()), 1)])
            .collect(),
    }
}

/// The list the wide and the long tables read their key's progress from.
const WIDE_LIST: &str = "inventory.list.fixture_wide";

/// Two tables at the contract's bounds of texts: `wide`, whose one key `k` has 57 ladders (a class
/// column `g` and seven columns of eight subs each), every column, sub and key named in 16
/// languages and every position said in 16; and `long`, a class column `p` whose class ladder has
/// 512 positions, each said in 16 languages. Built as the reader's result: its limits are the
/// reader's to check, not this test's.
fn bound_tables() -> StandardPackGoalTables {
    let mut columns = vec![GoalColumn {
        name: "g".to_owned(),
        read: GoalColumnRead::Class(vec!["r_g".to_owned()]),
        requires: Vec::new(),
    }];
    let mut reading = vec!["r_g".to_owned()];
    let mut key_columns = Vec::new();
    let mut column_names = vec![("g".to_owned(), long_text("g"))];
    let mut sub_names = Vec::new();
    let mut positions = vec![("g".to_owned(), long_said())];
    for column in 1..=7 {
        let name = format!("c{column}");
        let subs = (1..=8)
            .map(|sub| (format!("s{sub}"), vec![format!("r_{column}_{sub}")]))
            .collect::<Vec<_>>();
        reading.extend(subs.iter().map(|(_, read)| read[0].clone()));
        sub_names.push((
            name.clone(),
            subs.iter()
                .map(|(sub, _)| (sub.clone(), long_text(sub)))
                .collect(),
        ));
        key_columns.push((
            name.clone(),
            GoalKeyColumn::Subs(
                subs.iter()
                    .map(|(sub, _)| (sub.clone(), counted_ladder(2)))
                    .collect(),
            ),
        ));
        column_names.push((name.clone(), long_text(&name)));
        positions.push((name.clone(), long_said()));
        columns.push(GoalColumn {
            name,
            read: GoalColumnRead::Sub(subs),
            requires: Vec::new(),
        });
    }
    let wide = GoalTable {
        name: "wide".to_owned(),
        progress: vec![GoalProgressList {
            fact_key: WIDE_LIST.to_owned(),
            key_column: "id".to_owned(),
            held_by: false,
            columns: reading,
        }],
        columns,
        classes: vec![GoalClass {
            name: "c".to_owned(),
            ladders: vec![("g".to_owned(), counted_ladder(2))],
        }],
        keys: vec![GoalKey {
            name: "k".to_owned(),
            class: "c".to_owned(),
            own: Vec::new(),
            columns: key_columns,
        }],
    };
    let long = GoalTable {
        name: "long".to_owned(),
        progress: vec![GoalProgressList {
            fact_key: WIDE_LIST.to_owned(),
            key_column: "id".to_owned(),
            held_by: false,
            columns: vec!["r_p".to_owned()],
        }],
        columns: vec![GoalColumn {
            name: "p".to_owned(),
            read: GoalColumnRead::Class(vec!["r_p".to_owned()]),
            requires: Vec::new(),
        }],
        classes: vec![GoalClass {
            name: "c".to_owned(),
            ladders: vec![("p".to_owned(), counted_ladder(512))],
        }],
        keys: vec![GoalKey {
            name: "k".to_owned(),
            class: "c".to_owned(),
            own: Vec::new(),
            columns: Vec::new(),
        }],
    };
    StandardPackGoalTables {
        game: GAME.to_owned(),
        items: vec![GoalTableItem {
            name: "gold".to_owned(),
            pool: Some("pool.gold".to_owned()),
            recipe: None,
        }],
        tables: vec![wide, long],
        names: GoalTableNames {
            items: Vec::new(),
            tables: vec![
                (
                    "wide".to_owned(),
                    GoalTableNaming {
                        keys: vec![("k".to_owned(), long_text("k"))],
                        columns: column_names,
                        subs: sub_names,
                        positions,
                        ..GoalTableNaming::default()
                    },
                ),
                (
                    "long".to_owned(),
                    GoalTableNaming {
                        positions: vec![("p".to_owned(), long_said())],
                        ..GoalTableNaming::default()
                    },
                ),
            ],
        },
    }
}

/// The root of [`bound_tables`]: the records entries `wide` and `long`, each on its table.
fn bound_fixture() -> Fixture {
    let document = json!({"schema_version": "actingcommand.goals.v3", "game": GAME,
    "entries": [
        {"name": "wide", "type": "records", "table": "wide", "max_records": 1,
         "title": {"en": "Wide"}},
        {"name": "long", "type": "records", "table": "long", "max_records": 1,
         "title": {"en": "Long"}}
    ]});
    Fixture::of(goals_of(&document), bound_tables())
}

// The row cap (the study's 3.0 ruling 3): a record of `k` that raises each of its 57 ladders one
// position, its texts in 16 languages at every lane, would pass 256 KiB with them; it is answered
// without them (`texts_left_out`), every lane still there with its target and current position, and
// stays under the cap; its entry row is answered as any.
#[test]
fn rr4b1_p3_a_record_row_past_its_cap_is_answered_without_its_texts() {
    let fixture = bound_fixture();
    let mut to = BTreeMap::<String, Value>::new();
    to.insert("g".to_owned(), json!([1]));
    for column in 1..=7 {
        let subs = (1..=8)
            .map(|sub| (format!("s{sub}"), json!([1])))
            .collect::<BTreeMap<_, _>>();
        to.insert(format!("c{column}"), json!(subs));
    }
    let to = serde_json::to_string(&to).expect("the target's text");
    let records = stored(&[("wide", "k", to.as_str())]);
    let mut row = BTreeMap::from([("id".to_owned(), text("k"))]);
    row.insert("r_g".to_owned(), FactScalar::Integer(0));
    row.insert("r_p".to_owned(), FactScalar::Integer(0));
    for column in 1..=7 {
        for sub in 1..=8 {
            row.insert(format!("r_{column}_{sub}"), FactScalar::Integer(0));
        }
    }
    let lists = sources(vec![list(WIDE_LIST, vec![row])]);
    let page = fixture
        .read(
            &GoalPagePart::Records,
            &records,
            &lists,
            None,
            None,
            fixture.seq(1),
        )
        .expect("the records' page");
    assert_eq!(shape(&page), ["entry:wide", "record:wide/k", "entry:long"]);
    let rows = rows_of(&page);
    let stripped = &rows[1];
    assert_eq!(stripped["texts_left_out"], json!(true));
    assert!(stripped.get("key_name").is_none(), "{stripped}");
    let lanes = stripped["lanes"].as_array().expect("the lanes");
    assert_eq!(lanes.len(), 57);
    for lane in lanes {
        assert!(lane.get("column_name").is_none() && lane.get("sub_name").is_none());
        assert_eq!(
            lane["target"],
            json!({"index": 1, "position": [1]}),
            "{lane}"
        );
        assert_eq!(
            lane["current"],
            json!({"index": 0, "position": [0]}),
            "{lane}"
        );
    }
    let bytes = serde_json::to_vec(&page.rows[1])
        .expect("the row's bytes")
        .len();
    assert!(
        bytes <= MAX_GOAL_PAGE_VIEW_ROW_BYTES,
        "{bytes} bytes past the row cap"
    );
    assert!(rows[0].get("texts_left_out").is_none());
}

// The page rule (the study's 3.0 ruling 3): the 512 positions of `long`'s class ladder, each said in
// 16 languages, pass 768 KiB together. The first page holds as many as fit (more than one), is cut
// `bytes`, stays under the bound with its `next`; the next page holds the rest, without `next` or
// `cut`; together they are the ladder in its order. A limit of 100 is cut `count` after 100 rows. A
// cursor past the part's rows is no cursor of it.
#[test]
fn rr4b1_p4_a_page_is_cut_by_bytes_and_by_count_and_next_is_there_exactly_when_rows_remain() {
    let fixture = bound_fixture();
    let records = stored(&[]);
    let lists = sources(Vec::new());
    let part = GoalPagePart::Positions {
        entry: "long".to_owned(),
        ladder: GoalLadderRef::Class {
            class: "c".to_owned(),
            column: "p".to_owned(),
        },
    };
    let first = fixture
        .read(&part, &records, &lists, None, None, fixture.seq(0))
        .expect("the first page");
    let first_rows = first.rows.len();
    assert!(
        first_rows > 1 && first_rows < 512,
        "{first_rows} rows on the first page"
    );
    assert_eq!(first.rows_total, 512);
    assert_eq!(first.cut, Some(GoalPageCut::Bytes));
    let next = first.next.clone().expect("rows remain");
    assert_eq!(next.after, u32::try_from(first_rows).expect("a count"));
    assert_eq!(next.records_version, None);
    assert_eq!(next.part_sha256, part.digest());
    let bytes = serde_json::to_vec(&first).expect("the page's bytes").len();
    assert!(bytes <= MAX_GOAL_PAGE_VIEW_BYTES, "{bytes} bytes");
    assert_eq!(
        serde_json::to_value(first.ladder.as_ref()).expect("the ladder"),
        json!({"source": "class", "class": "c", "positions": 512})
    );
    let second = fixture
        .read(&part, &records, &lists, Some(&next), None, fixture.seq(0))
        .expect("the next page");
    assert_eq!((second.next.as_ref(), second.cut), (None, None));
    let indices = rows_of(&first)
        .iter()
        .chain(rows_of(&second).iter())
        .map(|row| row["index"].as_u64().expect("an index"))
        .collect::<Vec<_>>();
    assert_eq!(indices, (0..512).collect::<Vec<u64>>());
    assert_eq!(
        rows_of(&second)[0]["said"]["locale_00"],
        json!(format!("{} {first_rows}", "y".repeat(110)))
    );

    let counted = fixture
        .read(&part, &records, &lists, None, Some(100), fixture.seq(0))
        .expect("a page of 100");
    assert_eq!(counted.rows.len(), 100);
    assert_eq!(counted.cut, Some(GoalPageCut::Count));
    assert_eq!(counted.next.as_ref().map(|next| next.after), Some(100));

    let mut past = next;
    past.after = 513;
    assert_eq!(
        fixture.read(&part, &records, &lists, Some(&past), None, fixture.seq(0)),
        Err(PageFailure::Invalid)
    );
}

// The cursor (ruling 5 and Addendum 1). A cursor of `records` refuses, by its own word, a page over
// other stored records (`RecordsMoved`) and one over another declaration: the tables' sha256 or the
// root's records entries (`DeclarationMoved`). A cursor of `options` or of `positions` carries no
// records version: a records edit between two of their pages is answered with the next page; a
// declaration that moved refuses it. A cursor of one part given to another, a records version on a
// cursor of `options` or none on one of `records`, a limit of 0 or past 512 and a name of no form
// are the request's own refusal, `invalid_goal_page_view`, before the Runtime reads anything.
#[test]
fn rr4b1_p5_a_cursor_refuses_each_moved_source_with_its_own_word() {
    let fixture = Fixture::units();
    let records = stored(&[
        (ENTRY, "unit_a", r#"{"skill":[2]}"#),
        (ENTRY, "unit_b", r#"{"grade":[1]}"#),
    ]);
    let lists = sources(vec![
        roster(&[("unit_a", 0), ("unit_b", 0)]),
        skills(&[("unit_a", 1, 0)]),
    ]);
    let moved_tables = PageState {
        tables_sha256: Some(format!("sha256:{}", "b".repeat(64))),
        ..fixture.seq(3)
    };
    let moved_entries = PageState {
        entries_sha256: entries_digest(Some(&goals_of(&goals_document(9)))),
        ..fixture.seq(3)
    };
    assert_ne!(moved_entries.entries_sha256, fixture.seq(3).entries_sha256);

    let records_part = GoalPagePart::Records;
    let first = fixture
        .read(
            &records_part,
            &records,
            &lists,
            None,
            Some(2),
            fixture.seq(3),
        )
        .expect("the first page of records");
    let at_records = first.next.clone().expect("rows remain");
    assert_eq!(at_records.records_version, Some(3));
    for (name, now, refused) in [
        ("records", fixture.seq(4), PageFailure::RecordsMoved),
        (
            "tables",
            moved_tables.clone(),
            PageFailure::DeclarationMoved,
        ),
        (
            "entries",
            moved_entries.clone(),
            PageFailure::DeclarationMoved,
        ),
    ] {
        assert_eq!(
            fixture.read(
                &records_part,
                &records,
                &lists,
                Some(&at_records),
                None,
                now
            ),
            Err(refused),
            "{name}"
        );
    }
    let again = fixture
        .read(
            &records_part,
            &records,
            &lists,
            Some(&at_records),
            None,
            fixture.seq(3),
        )
        .expect("the next page over the same state");
    assert_eq!(again.rows.len(), 3);

    let options = GoalPagePart::Options {
        entry: ENTRY.to_owned(),
    };
    let positions = GoalPagePart::Positions {
        entry: ENTRY.to_owned(),
        ladder: GoalLadderRef::Class {
            class: "all".to_owned(),
            column: "grade".to_owned(),
        },
    };
    for part in [&options, &positions] {
        let first = fixture
            .read(part, &records, &lists, None, Some(1), fixture.seq(3))
            .expect("the first page");
        let cursor = first.next.clone().expect("rows remain");
        assert_eq!(cursor.records_version, None, "{part:?}");
        let next = fixture
            .read(
                part,
                &records,
                &lists,
                Some(&cursor),
                Some(1),
                fixture.seq(4),
            )
            .expect("a records edit moves nothing this part's cursor compares");
        assert_eq!(next.rows.len(), 1, "{part:?}");
        assert_eq!(
            next.next.as_ref().map(|next| next.after),
            Some(2),
            "{part:?}"
        );
        for now in [moved_tables.clone(), moved_entries.clone()] {
            assert_eq!(
                fixture.read(part, &records, &lists, Some(&cursor), None, now),
                Err(PageFailure::DeclarationMoved),
                "{part:?}"
            );
        }
    }

    let options_cursor = fixture
        .read(&options, &records, &lists, None, Some(1), fixture.seq(3))
        .expect("a page of options")
        .next
        .expect("rows remain");
    let request = |part: GoalPagePart, cursor: Option<GoalPageCursor>, limit: Option<u32>| {
        RuntimeOperation::GoalPageView {
            instance_alias: ALIAS.to_owned(),
            part,
            cursor,
            limit,
        }
        .validate()
    };
    let refused = |result: actingcommand_contract::RuntimeContractResult<()>, case: &str| {
        let error = result.expect_err(case);
        assert_eq!(
            error.code().as_str(),
            RuntimeContractCode::InvalidGoalPageView.as_str(),
            "{case}"
        );
    };
    refused(
        request(GoalPagePart::Records, Some(options_cursor.clone()), None),
        "a cursor of options given to records",
    );
    let mut without_version = at_records.clone();
    without_version.records_version = None;
    refused(
        request(GoalPagePart::Records, Some(without_version), None),
        "a records cursor without its version",
    );
    let mut with_version = options_cursor.clone();
    with_version.records_version = Some(3);
    refused(
        request(options.clone(), Some(with_version), None),
        "an options cursor with a records version",
    );
    let mut bad_digest = options_cursor.clone();
    bad_digest.entries_sha256 = "sha256:short".to_owned();
    refused(
        request(options.clone(), Some(bad_digest), None),
        "a digest of no form",
    );
    refused(request(options.clone(), None, Some(0)), "a limit of 0");
    refused(request(options.clone(), None, Some(513)), "a limit of 513");
    refused(
        request(
            GoalPagePart::Options {
                entry: "Not A Name".to_owned(),
            },
            None,
            None,
        ),
        "an entry of no name's form",
    );
    assert_eq!(
        request(options.clone(), Some(options_cursor), Some(512)),
        Ok(())
    );
    assert_eq!(
        request(GoalPagePart::Records, Some(at_records), None),
        Ok(())
    );
}

// Options, positions and what the root does not have (ruling 2, the study's 3.1). The options of
// `raise`: the table's columns (`grade` named, `skill` with its prerequisite said in both of the
// pack's languages and its condition named, `mastery` with its sub named), its class, then its keys:
// `unit_a` named and held, each of its ladders with its size and current position; `unit_b` held,
// its skill not known (no row of it in the skills list); `unit_c`, which the roster does not hold,
// with its ladders and no current position; the page's freshness the lists'. The positions of the
// class's grade ladder (said), of `unit_a`'s mastery sub (said by the column's list) and of its
// grade (the class's ladder). Each refusal with the edit's word at its pointer.
#[test]
fn rr4b1_p6_options_and_positions_and_what_the_root_does_not_have() {
    let fixture = Fixture::units();
    let records = stored(&[]);
    let lists = sources(vec![
        roster(&[("unit_a", 0), ("unit_b", 0)]),
        skills(&[("unit_a", 1, 0)]),
    ]);
    let read =
        |part: &GoalPagePart| fixture.read(part, &records, &lists, None, None, fixture.seq(0));
    let options = read(&GoalPagePart::Options {
        entry: ENTRY.to_owned(),
    })
    .expect("the options");
    assert_eq!(
        shape(&options),
        [
            "column:grade",
            "column:skill",
            "column:mastery",
            "class:all",
            "key:unit_a",
            "key:unit_b",
            "key:unit_c",
        ]
    );
    assert_eq!((options.rows_total, options.next.as_ref()), (7, None));
    assert_eq!(options.fresh_until_unix_ms, Some(READ_AT + FRESH_MS));
    let rows = rows_of(&options);
    let grade_said =
        |grade: i64| json!({"en": format!("Grade {grade}"), "fr": format!("Rang {grade}")});
    assert_eq!(
        rows[..4],
        [
            json!({"row": "column", "column": "grade", "name": {"en": "Grade"}, "per": "class",
                   "requires": []}),
            json!({"row": "column", "column": "skill", "per": "key", "requires": [
                {"column": "grade", "at_least": [2], "at_least_said": grade_said(2),
                 "from": [3]},
                {"condition": "side_story", "name": {"en": "The side story"}}]}),
            json!({"row": "column", "column": "mastery", "per": "sub",
                   "subs": [{"sub": "1", "name": {"en": "First mastery"}}], "requires": []}),
            json!({"row": "class", "class": "all", "name": {"en": "Everyone"}}),
        ]
    );
    let held = json!({"state": "held", "fact_key": ROSTER});
    assert_eq!(
        rows[4],
        json!({"row": "key", "key": "unit_a", "name": {"en": "First unit"}, "class": "all",
               "held": held, "lanes": [
                   {"column": "grade", "source": "class", "positions": 3,
                    "current": {"index": 0, "position": [0]}},
                   {"column": "skill", "source": "own", "positions": 3,
                    "current": {"index": 0, "position": [1]}},
                   {"column": "mastery", "sub": "1", "source": "own", "positions": 2,
                    "current": {"index": 0, "position": [0]}}]})
    );
    assert_eq!(
        rows[5],
        json!({"row": "key", "key": "unit_b", "class": "all", "held": held, "lanes": [
                   {"column": "grade", "source": "class", "positions": 3,
                    "current": {"index": 0, "position": [0]}},
                   {"column": "skill", "source": "own", "positions": 3,
                    "unknown": {"code": "goal_records_row_missing", "fact_key": SKILLS}}]})
    );
    assert_eq!(
        rows[6],
        json!({"row": "key", "key": "unit_c", "class": "all",
               "held": {"state": "not_held", "fact_key": ROSTER}, "lanes": [
                   {"column": "grade", "source": "class", "positions": 3},
                   {"column": "skill", "source": "own", "positions": 3}]})
    );

    let class_grade = read(&GoalPagePart::Positions {
        entry: ENTRY.to_owned(),
        ladder: GoalLadderRef::Class {
            class: "all".to_owned(),
            column: "grade".to_owned(),
        },
    })
    .expect("the class's grade ladder");
    assert_eq!(
        rows_of(&class_grade),
        (0..3)
            .map(|grade| {
                json!({"row": "position", "index": grade, "position": [grade],
                       "said": grade_said(grade)})
            })
            .collect::<Vec<_>>()
    );
    assert_eq!(
        serde_json::to_value(class_grade.ladder.as_ref()).expect("the ladder"),
        json!({"source": "class", "class": "all", "positions": 3})
    );
    let mastery = read(&GoalPagePart::Positions {
        entry: SECOND.to_owned(),
        ladder: GoalLadderRef::Key {
            key: "unit_a".to_owned(),
            column: "mastery".to_owned(),
            sub: Some("1".to_owned()),
        },
    })
    .expect("unit_a's mastery sub");
    assert_eq!(
        rows_of(&mastery),
        [
            json!({"row": "position", "index": 0, "position": [0],
                   "said": {"en": "Mastery 0"}}),
            json!({"row": "position", "index": 1, "position": [1],
                   "said": {"en": "Mastery 1"}}),
        ]
    );
    assert_eq!(
        serde_json::to_value(mastery.ladder.as_ref()).expect("the ladder"),
        json!({"source": "own", "positions": 2})
    );
    let key_grade = read(&GoalPagePart::Positions {
        entry: ENTRY.to_owned(),
        ladder: GoalLadderRef::Key {
            key: "unit_a".to_owned(),
            column: "grade".to_owned(),
            sub: None,
        },
    })
    .expect("unit_a's grade");
    assert_eq!(
        serde_json::to_value(key_grade.ladder.as_ref()).expect("the ladder"),
        json!({"source": "class", "class": "all", "positions": 3})
    );

    let refused =
        |problem: GoalProblem, pointer: &'static str| PageFailure::Refused { problem, pointer };
    let positions = |ladder: GoalLadderRef| GoalPagePart::Positions {
        entry: ENTRY.to_owned(),
        ladder,
    };
    let key = |key: &str, column: &str, sub: Option<&str>| GoalLadderRef::Key {
        key: key.to_owned(),
        column: column.to_owned(),
        sub: sub.map(str::to_owned),
    };
    let cases = [
        (
            "a records entry whose table the file does not have",
            GoalPagePart::Options {
                entry: "spare".to_owned(),
            },
            refused(GoalProblem::NameUnknown, "/part/entry"),
        ),
        (
            "no records entry of the name",
            GoalPagePart::Options {
                entry: "intro".to_owned(),
            },
            refused(GoalProblem::NameUnknown, "/part/entry"),
        ),
        (
            "a key the table does not hold",
            positions(key("unit_z", "grade", None)),
            refused(GoalProblem::NotAnOption, "/part/ladder/key"),
        ),
        (
            "a class the table does not hold",
            positions(GoalLadderRef::Class {
                class: "none".to_owned(),
                column: "grade".to_owned(),
            }),
            refused(GoalProblem::NotAnOption, "/part/ladder/class"),
        ),
        (
            "a column the class has no ladder of",
            positions(GoalLadderRef::Class {
                class: "all".to_owned(),
                column: "skill".to_owned(),
            }),
            refused(GoalProblem::OutOfRange, "/part/ladder/column"),
        ),
        (
            "a sub the key has no ladder of",
            positions(key("unit_b", "mastery", Some("1"))),
            refused(GoalProblem::OutOfRange, "/part/ladder/sub"),
        ),
        (
            "a column of per sub without its sub",
            positions(key("unit_a", "mastery", None)),
            refused(GoalProblem::OutOfRange, "/part/ladder/column"),
        ),
    ];
    for (name, part, expected) in cases {
        assert_eq!(read(&part), Err(expected), "{name}");
    }
}

// ---- The host part ----

const INSTANCE_A: &str = POLICY_INSTANCE_ALIAS;
const INSTANCE_B: &str = POLICY_INSTANCE_ALIAS_B;
const T0: u64 = POLICY_NOW_UNIX_MS;
/// The root's one rule and its package.
const TASK: &str = "fixture.records";
/// The `resource` entry of the root's `goals.json`, on [`POOL_A`].
const STOCK: &str = "stock";
/// The `records` entry of the root's `goals.json`, on [`HEROES`], at most two records.
const RECORDS: &str = "raise";
/// The table of the root's `goal-tables.json` the records entry names.
const HEROES: &str = "heroes";
/// The table's progress list, its `held_by` list.
const HEROES_LIST: &str = "inventory.list.fixture_heroes";
const VIEWED: &str = "request_goal_view_completed";
const PAGED: &str = "request_goal_page_view_completed";
const EDITED: &str = "request_edit_goal_completed";
/// The default `standard_packs.check_interval_ms`: a check reads the roots again after it.
const CHECK_INTERVAL_MS: u64 = 60_000;

/// `hero_a`: of the class `common` (whose `grade` ladder is `[0], [1], [2]`), with its own `skill`
/// ladder `[1], [2], [3]` and a `mastery` ladder `[0], [1]` for its sub `1` (`gate_455_rr2`'s).
fn hero_a() -> Value {
    json!({"class": "common",
           "skill": {"positions": [[1], [2], [3]], "steps": [{"coin": 5}, {"coin": 6}]},
           "mastery": {"1": {"positions": [[0], [1]], "steps": [{"coin": 7}]}}})
}

/// The table `heroes` (`gate_455_rr2`'s): the columns `grade` (per class), `skill` (per key) and
/// `mastery` (per sub, one sub `1`), the class `common` and the keys `hero_a` and, with `with_b`,
/// `hero_b` (of the class alone).
fn heroes(with_b: bool) -> Value {
    let mut keys = serde_json::Map::new();
    keys.insert("hero_a".to_owned(), hero_a());
    if with_b {
        keys.insert("hero_b".to_owned(), json!({"class": "common"}));
    }
    json!({
        "name": HEROES,
        "progress": [{"fact_key": HEROES_LIST, "key_column": "hero",
                      "held_by": true, "columns": ["grade", "skill", "mastery_1"]}],
        "columns": [{"name": "grade", "read": ["grade"], "per": "class"},
                    {"name": "skill", "read": ["skill"], "per": "key"},
                    {"name": "mastery", "per": "sub", "subs": {"1": ["mastery_1"]}}],
        "classes": {"common": {"grade": {"positions": [[0], [1], [2]],
                                         "steps": [{"coin": 10}, {"coin": 20}]}}},
        "keys": keys
    })
}

fn tables_file(tables: Vec<Value>) -> Value {
    json!({
        "schema_version": "actingcommand.goal-tables.v1",
        "game": GAME_A,
        "items": {"coin": {"pool": POOL_A}},
        "tables": tables
    })
}

/// The root's `goals.json` (version 3): the `resource` entry [`STOCK`] and, with `records`, the
/// records entry [`RECORDS`] on [`HEROES`], at most two records.
fn goals(records: bool) -> Value {
    let mut entries = vec![
        json!({"name": STOCK, "type": "resource", "governs": {"pool": POOL_A},
                                  "title": {"en": "Stock"}}),
    ];
    if records {
        entries.push(json!({"name": RECORDS, "type": "records", "table": HEROES,
                            "max_records": 2, "title": {"en": "Whom to raise"}}));
    }
    json!({"schema_version": "actingcommand.goals.v3", "game": GAME_A, "entries": entries})
}

fn goals_path(packs: &Path) -> PathBuf {
    packs.join(GAME_A).join("goals.json")
}

fn tables_path(packs: &Path) -> PathBuf {
    packs.join(GAME_A).join("goal-tables.json")
}

/// `gate_455_rr2`'s game root below `packs`: its bundle, its `schedule.json` with the one rule (due
/// at every evaluation, producing the pool, whose valuation names its step and its importance), the
/// package, `goals.json` with both entries and `goal-tables.json` with `heroes` of both keys.
fn place_game_root(packs: &Path) {
    let game = packs.join(GAME_A);
    write_json(
        &game.join("bundle.json"),
        &bundle(GAME_A, SERVER_A, &[TASK]),
    );
    let mut produces = rule(TASK, TASK, 100, Some(POOL_A));
    produces["trigger"] = json!({
        "kind": "clock",
        "schedule": {"kind": "interval", "clock_source": {"kind": "local"}, "every_ms": 1,
                     "anchor_ms": 0}
    });
    produces["loop_budget"]["window_iteration_limit"] = json!(16);
    produces["loop_budget"]["max_runtime_ms"] = json!(3_600_000);
    let mut activity = profiles("", Some(POOL_A));
    for profile in &mut activity {
        profile["max_window_iterations"] = json!(16);
    }
    let mut valued = pool(POOL_A);
    valued["valuation"]["gap"] = json!({"rule": "shortfall_linear", "weight_milli": 100});
    write_json(
        &game.join("schedule.json"),
        &json!({
            "schema_version": "actingcommand.schedule.v1",
            "requires": [],
            "rules": [produces],
            "pools": [valued],
            "activity": activity,
            "timeline": [],
            "held": []
        }),
    );
    write_package(&game.join(TASK), TASK, None, None);
    write_json(&goals_path(packs), &goals(true));
    write_json(&tables_path(packs), &tables_file(vec![heroes(true)]));
}

/// The two instances of the game on its one server, each with its reading of the pool
/// (`resource.primary`): 10, a minute before the fixture's time.
fn facts() -> EvaluationFacts {
    let mut inputs = policy_facts();
    inputs.outcomes.clear();
    inputs.instances = [INSTANCE_A, INSTANCE_B]
        .into_iter()
        .map(|alias| InstanceSnapshot {
            instance_id: alias.to_owned(),
            server_id: SERVER_A.to_owned(),
            game_id: GAME_A.to_owned(),
            host_id: "fixture-host-a".to_owned(),
            available: true,
            capability_operation_ids: vec![
                actingcommand_policy::STANDARD_PACK_OPERATION_ID.to_owned(),
            ],
            preferred_task_ids: Vec::new(),
        })
        .collect();
    for alias in [INSTANCE_A, INSTANCE_B] {
        inputs.facts.push(actingcommand_policy::ObservedFact {
            scope: actingcommand_policy::ScopeSelector::Instance {
                instance_id: alias.to_owned(),
            },
            fact_key: "resource.primary".to_owned(),
            value: actingcommand_policy::FactValue::Integer(10),
            observed_at_unix_ms: T0 - 60_000,
            expires_at_unix_ms: Some(T0 + 86_400_000),
            confidence_milli: 1000,
        });
    }
    inputs
}

fn lab_request(client: &TestClient, operation: RuntimeOperation) -> RuntimeRequest {
    RuntimeRequest::new(
        client.ids.mint_request_id().expect("request id"),
        client.ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::Lab,
        EventSource::Lab,
        1,
        operation,
    )
    .expect("lab runtime request")
}

/// The host over the game root, its state root, the game root it reads and its clock.
struct Scene {
    _state: TempDir,
    packs: TempDir,
    clock: Arc<ManualRuntimeClock>,
    host: RuntimeHost,
}

impl Scene {
    fn new() -> Self {
        let state = TempDir::new().expect("state root");
        let packs = TempDir::new().expect("standard-pack root");
        place_game_root(packs.path());
        let clock = Arc::new(ManualRuntimeClock::new(T0, 0));
        let provider = FakeProvider::from_entries([INSTANCE_A, INSTANCE_B].map(|alias| {
            (
                alias.to_owned(),
                instance_id(),
                Arc::new(FakeState::default()),
            )
        }));
        let host = RuntimeHost::start(
            config(&state)
                .with_policy_inputs(PolicyInputSnapshot::new(facts(), policy_resources()))
                .with_runtime_clock(clock.clone())
                .with_standard_packs_root(Some(packs.path().to_owned()))
                .with_standard_pack_schedule(true),
            Arc::new(provider),
        )
        .expect("start the composer-mode host");
        Self {
            _state: state,
            packs,
            clock,
            host,
        }
    }

    /// One policy cycle at `at`, the Runtime clock moved there first; the first composes the root
    /// and its workbench check reads the root's goal files.
    fn cycle(&self, at: u64) {
        advance_manual_clock_to(&self.clock, at);
        self.host
            .capacity_sampler_for_test()
            .expect("capacity owner")()
        .expect("capacity sample");
        self.host
            .evaluate_policy_cycle_with_test_inputs(
                &facts(),
                &resources(&[POOL_A]),
                EvaluationTime {
                    unix_ms: at,
                    monotonic_ms: at,
                },
                7,
                PolicyTrigger::FactsChanged,
            )
            .unwrap_or_else(|error| panic!("evaluate the policy at {at}: {error}"))
            .evaluation
            .unwrap_or_else(|| panic!("a full evaluation at {at}"));
    }

    /// Publishes instance A's progress list of the heroes, `hero_a` at grade 0, skill 1 and its
    /// first mastery 0, read ten seconds before the fixture's time, fresh for a minute.
    fn publish_progress(&self) {
        let mut fact = stored_fact(
            FactScope::Instance {
                instance_id: INSTANCE_A.to_owned(),
            },
            HEROES_LIST,
            ContractFactValue::RecordList(vec![BTreeMap::from([
                ("hero".to_owned(), text("hero_a")),
                ("grade".to_owned(), FactScalar::Integer(0)),
                ("skill".to_owned(), FactScalar::Integer(1)),
                ("mastery_1".to_owned(), FactScalar::Integer(0)),
            ])]),
            "snapshot:rr4b1:heroes",
            Vec::new(),
        );
        fact.observed_at_unix_ms = T0 - 10_000;
        fact.expires_at_unix_ms = Some(T0 + 50_000);
        self.host
            .publish_list_reading_for_test(fact)
            .expect("the heroes' progress list");
    }

    /// `alias`'s goal page, read on a connection of the CLI.
    fn view(&self, alias: &str) -> GoalView {
        let mut client = TestClient::connect(&self.host);
        let request = client.request(RuntimeOperation::GoalView {
            instance_alias: alias.to_owned(),
        });
        let receipt = client.send(&request);
        assert_eq!(receipt.outcome().code().as_str(), VIEWED, "{receipt:?}");
        let Some(RuntimeResult::GoalViewed { view }) = receipt.result() else {
            panic!("a goal view: {receipt:?}");
        };
        (**view).clone()
    }

    /// `GoalPageView` of `part` on `alias`, read on a connection of the CLI.
    fn page(
        &self,
        alias: &str,
        part: &GoalPagePart,
        cursor: Option<&GoalPageCursor>,
        limit: Option<u32>,
    ) -> RuntimeReceipt {
        let mut client = TestClient::connect(&self.host);
        let request = client.request(RuntimeOperation::GoalPageView {
            instance_alias: alias.to_owned(),
            part: part.clone(),
            cursor: cursor.cloned(),
            limit,
        });
        client.send(&request)
    }

    /// `EditGoal` of `op` on `alias`, from the Lab bound with `Lab-CLI`, the Runtime clock at `at`
    /// first.
    fn edit(&self, alias: &str, op: GoalEdit, at: u64) -> RuntimeReceipt {
        advance_manual_clock_to(&self.clock, at);
        let mut client = TestClient::connect(&self.host);
        let bind = client.request(RuntimeOperation::BindChannel {
            channel: RuntimeChannel::Lab,
            via: None,
            initiator: Some(DeclaredInitiator::from(Initiator::LabCli)),
        });
        let bound = client.send(&bind);
        assert_eq!(bound.state(), RuntimeReceiptState::Completed, "{bound:?}");
        let request = lab_request(
            &client,
            RuntimeOperation::EditGoal {
                instance_alias: alias.to_owned(),
                expected_version: None,
                op,
            },
        );
        client.send(&request)
    }

    fn close(self) {
        assert!(
            self.host.fatal_error().expect("runtime health").is_none(),
            "the Runtime holds no fatal condition"
        );
        self.host.close().expect("close the composer-mode host");
    }
}

fn set_records(value: &Value) -> GoalEdit {
    GoalEdit::Set {
        name: RECORDS.to_owned(),
        value_json: value.to_string(),
    }
}

/// What a taken `EditGoal` answered.
#[track_caller]
fn taken(receipt: &RuntimeReceipt) -> GoalEdited {
    assert_eq!(receipt.outcome().code().as_str(), EDITED, "{receipt:?}");
    let Some(RuntimeResult::GoalEdited { edited }) = receipt.result() else {
        panic!("an edited goal: {receipt:?}");
    };
    edited.clone()
}

/// The page a completed `GoalPageView` answered.
#[track_caller]
fn paged(receipt: &RuntimeReceipt) -> GoalPageView {
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(receipt.outcome().code().as_str(), PAGED, "{receipt:?}");
    let Some(RuntimeResult::GoalPageViewed { page }) = receipt.result() else {
        panic!("a goal page: {receipt:?}");
    };
    (**page).clone()
}

/// A refused request: Denied with `code`, no result; its outcome's `key` value.
#[track_caller]
fn denied(receipt: &RuntimeReceipt, code: &str, key: &str) -> Option<Value> {
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    assert_eq!(receipt.outcome().code().as_str(), code, "{receipt:?}");
    assert!(receipt.result().is_none(), "{receipt:?}");
    receipt.outcome().value(key).cloned()
}

/// `hero_a` to grade `[2]`, skill `[3]` and mastery sub `1` at `[1]`.
fn raise_a() -> Value {
    json!([{"key": "hero_a", "to": {"grade": [2], "skill": [3], "mastery": {"1": [1]}}}])
}

/// The heroes' class grade ladder, read by `positions`.
fn grade_ladder() -> GoalPagePart {
    GoalPagePart::Positions {
        entry: RECORDS.to_owned(),
        ladder: GoalLadderRef::Class {
            class: "common".to_owned(),
            column: "grade".to_owned(),
        },
    }
}

fn hero_options() -> GoalPagePart {
    GoalPagePart::Options {
        entry: RECORDS.to_owned(),
    }
}

// Rulings 1 to 4 on the host, the study's host test. In composer mode, after the first cycle, the
// heroes' progress list published and `hero_a`'s record set from the Lab: the goal page still lists
// no records entry; `records`, read a row a page, answers the page's version (`GoalView`'s), its
// declaration, the tables accepted with their sha256, the entry (after `stock`, applied, one record)
// and then `hero_a`'s record, every lane with its target and its current position from the list,
// held; `options`, four rows a page, its three columns and its class, then its two keys, `hero_a`
// held with each ladder's size and current position and `hero_b`, whom the list does not hold,
// without; `positions` of the class's grade ladder and of `hero_a`'s mastery sub, by class and by
// key.
#[test]
fn rr4b1_h1_records_options_and_positions_are_read_in_pages_beside_an_unchanged_goal_page() {
    let scene = Scene::new();
    scene.cycle(T0);
    scene.publish_progress();
    let set = taken(&scene.edit(INSTANCE_A, set_records(&raise_a()), T0 + 1_000));
    let view = scene.view(INSTANCE_A);
    assert_eq!(view.version, set.version);
    assert!(
        !view.entries.iter().any(|entry| entry.name == RECORDS),
        "{view:?}"
    );

    let first = paged(&scene.page(INSTANCE_A, &GoalPagePart::Records, None, Some(1)));
    assert_eq!(first.version, view.version);
    assert_eq!(first.declaration, view.declaration);
    assert_eq!(first.tables.state, GoalTablesRead::Accepted);
    assert!(
        first
            .tables
            .sha256
            .as_deref()
            .is_some_and(|sha256| sha256.starts_with("sha256:")),
        "{:?}",
        first.tables
    );
    assert_eq!(first.records_state, Some(GoalPageRecordsState::Read));
    assert_eq!((first.rows_total, first.cut), (2, Some(GoalPageCut::Count)));
    let rows = rows_of(&first);
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (
            &rows[0]["name"],
            &rows[0]["after_entry"],
            &rows[0]["applies"],
            &rows[0]["records_total"]
        ),
        (&json!(RECORDS), &json!(STOCK), &json!(true), &json!(1))
    );
    let next = first.next.clone().expect("the record remains");
    assert_eq!(next.records_version, Some(set.version));
    let second = paged(&scene.page(INSTANCE_A, &GoalPagePart::Records, Some(&next), Some(1)));
    assert_eq!((second.next.as_ref(), second.cut), (None, None));
    assert_eq!(
        rows_of(&second),
        [json!({"row": "record", "entry": RECORDS, "key": "hero_a",
                "to": {"grade": [2], "mastery": {"1": [1]}, "skill": [3]},
                "set": {"unix_ms": T0 + 1_000, "channel": "lab", "initiator": "Lab-CLI"},
                "applied": true,
                "held": {"state": "held", "fact_key": HEROES_LIST},
                "lanes": [
                    {"column": "grade", "source": "class",
                     "target": {"index": 2, "position": [2]},
                     "current": {"index": 0, "position": [0]}},
                    {"column": "mastery", "sub": "1", "source": "own",
                     "target": {"index": 1, "position": [1]},
                     "current": {"index": 0, "position": [0]}},
                    {"column": "skill", "source": "own",
                     "target": {"index": 2, "position": [3]},
                     "current": {"index": 0, "position": [1]}}]})]
    );

    let options = paged(&scene.page(INSTANCE_A, &hero_options(), None, Some(4)));
    assert_eq!(
        shape(&options),
        [
            "column:grade",
            "column:skill",
            "column:mastery",
            "class:common"
        ]
    );
    assert_eq!(
        (options.rows_total, options.cut),
        (6, Some(GoalPageCut::Count))
    );
    let keys = paged(&scene.page(INSTANCE_A, &hero_options(), options.next.as_ref(), Some(4)));
    assert_eq!(keys.next, None);
    assert_eq!(
        rows_of(&keys),
        [
            json!({"row": "key", "key": "hero_a", "class": "common",
                   "held": {"state": "held", "fact_key": HEROES_LIST}, "lanes": [
                       {"column": "grade", "source": "class", "positions": 3,
                        "current": {"index": 0, "position": [0]}},
                       {"column": "skill", "source": "own", "positions": 3,
                        "current": {"index": 0, "position": [1]}},
                       {"column": "mastery", "sub": "1", "source": "own", "positions": 2,
                        "current": {"index": 0, "position": [0]}}]}),
            json!({"row": "key", "key": "hero_b", "class": "common",
                   "held": {"state": "not_held", "fact_key": HEROES_LIST}, "lanes": [
                       {"column": "grade", "source": "class", "positions": 3}]}),
        ]
    );

    let grade = paged(&scene.page(INSTANCE_A, &grade_ladder(), None, None));
    assert_eq!(shape(&grade), ["position:0", "position:1", "position:2"]);
    assert_eq!(
        serde_json::to_value(grade.ladder.as_ref()).expect("the ladder"),
        json!({"source": "class", "class": "common", "positions": 3})
    );
    let mastery = paged(&scene.page(
        INSTANCE_A,
        &GoalPagePart::Positions {
            entry: RECORDS.to_owned(),
            ladder: GoalLadderRef::Key {
                key: "hero_a".to_owned(),
                column: "mastery".to_owned(),
                sub: Some("1".to_owned()),
            },
        },
        None,
        None,
    ));
    assert_eq!(
        rows_of(&mastery),
        [
            json!({"row": "position", "index": 0, "position": [0]}),
            json!({"row": "position", "index": 1, "position": [1]}),
        ]
    );
    scene.close();
}

// Ruling 5 and Addendum 1 on the host. Pages of `records` (a row a page), `options` (four) and
// `positions` (one) are read; then a records edit: the next page of `records` is
// `goal_page_view_records_moved` with the page's version now, while the next pages of `options`
// and `positions` are answered (their cursors carry no records version) with the new version; then
// the root's `goal-tables.json` changes and the workbench reads it again: the cursor of `options`
// is `goal_page_view_declaration_moved` with the same version, and so is a cursor of `records`
// read after the edit.
#[test]
fn rr4b1_h2_a_records_edit_moves_the_records_cursor_alone_and_a_new_tables_file_moves_every_cursor()
{
    let scene = Scene::new();
    scene.cycle(T0);
    scene.publish_progress();
    taken(&scene.edit(INSTANCE_A, set_records(&raise_a()), T0 + 1_000));
    let records = paged(&scene.page(INSTANCE_A, &GoalPagePart::Records, None, Some(1)));
    let options = paged(&scene.page(INSTANCE_A, &hero_options(), None, Some(4)));
    let grade = paged(&scene.page(INSTANCE_A, &grade_ladder(), None, Some(1)));

    let edited = taken(&scene.edit(
        INSTANCE_A,
        set_records(&json!([{"key": "hero_b", "to": {"grade": [1]}}])),
        T0 + 2_000,
    ));
    assert_eq!(
        denied(
            &scene.page(
                INSTANCE_A,
                &GoalPagePart::Records,
                records.next.as_ref(),
                Some(1)
            ),
            "goal_page_view_records_moved",
            "current_version"
        ),
        Some(json!(edited.version))
    );
    let keys = paged(&scene.page(INSTANCE_A, &hero_options(), options.next.as_ref(), Some(4)));
    assert_eq!(shape(&keys), ["key:hero_a", "key:hero_b"]);
    assert_eq!(keys.version, edited.version);
    let second = paged(&scene.page(INSTANCE_A, &grade_ladder(), grade.next.as_ref(), Some(1)));
    assert_eq!(shape(&second), ["position:1"]);
    assert_eq!(second.next.as_ref().map(|next| next.after), Some(2));
    let after_edit = paged(&scene.page(INSTANCE_A, &GoalPagePart::Records, None, Some(1)));

    write_json(
        &tables_path(scene.packs.path()),
        &tables_file(vec![heroes(false)]),
    );
    scene.cycle(T0 + CHECK_INTERVAL_MS);
    assert_eq!(
        denied(
            &scene.page(INSTANCE_A, &hero_options(), options.next.as_ref(), Some(4)),
            "goal_page_view_declaration_moved",
            "current_version"
        ),
        Some(json!(edited.version))
    );
    assert_eq!(
        denied(
            &scene.page(
                INSTANCE_A,
                &GoalPagePart::Records,
                after_edit.next.as_ref(),
                Some(1)
            ),
            "goal_page_view_declaration_moved",
            "current_version"
        ),
        Some(json!(edited.version))
    );
    scene.close();
}

// A page not in memory, an undeclared name, what the root does not have and a cursor past the
// part (the study's host test). Before the first cycle `records` answers `not_checked_yet` and no
// row, as `GoalView` does. After it, `options` of a name no records entry has is
// `goal_page_view_refused` with `name_unknown` at `/part/entry`, and a `records` cursor past the
// part's rows `invalid_goal_page_view`. Once the root's `goals.json` no longer declares the records
// entry and the workbench read it again, `records` lists the stored records under their name
// (`declared: false`), not applied with the edit's word. An alias no instance has is
// `instance_unknown`.
#[test]
fn rr4b1_h3_a_page_not_in_memory_an_undeclared_name_and_what_the_root_does_not_have() {
    let scene = Scene::new();
    let unchecked = paged(&scene.page(INSTANCE_A, &GoalPagePart::Records, None, None));
    assert_eq!(
        unchecked.declaration.state,
        GoalDeclarationRead::NotCheckedYet
    );
    assert_eq!(
        (
            unchecked.rows.len(),
            unchecked.rows_total,
            unchecked.next.as_ref()
        ),
        (0, 0, None)
    );

    scene.cycle(T0);
    taken(&scene.edit(
        INSTANCE_A,
        set_records(&json!([{"key": "hero_a", "to": {"grade": [2]}}])),
        T0 + 1_000,
    ));
    let receipt = scene.page(
        INSTANCE_A,
        &GoalPagePart::Options {
            entry: "spare".to_owned(),
        },
        None,
        None,
    );
    assert_eq!(
        denied(&receipt, "goal_page_view_refused", "goal_problem"),
        Some(json!("name_unknown"))
    );
    assert_eq!(
        receipt.outcome().value("json_pointer"),
        Some(&json!("/part/entry"))
    );
    let first = paged(&scene.page(INSTANCE_A, &GoalPagePart::Records, None, Some(1)));
    let mut past = first.next.clone().expect("rows remain");
    past.after = 99;
    denied(
        &scene.page(INSTANCE_A, &GoalPagePart::Records, Some(&past), None),
        "invalid_goal_page_view",
        "operation",
    );

    write_json(&goals_path(scene.packs.path()), &goals(false));
    scene.cycle(T0 + CHECK_INTERVAL_MS);
    let undeclared = paged(&scene.page(INSTANCE_A, &GoalPagePart::Records, None, None));
    assert_eq!(
        rows_of(&undeclared),
        [
            json!({"row": "entry", "name": RECORDS, "declared": false, "applies": false,
                   "problem": "name_unknown", "records_total": 1}),
            json!({"row": "record", "entry": RECORDS, "key": "hero_a",
                   "to": {"grade": [2]},
                   "set": {"unix_ms": T0 + 1_000, "channel": "lab", "initiator": "Lab-CLI"},
                   "applied": false, "problem": "name_unknown", "lanes": []}),
        ]
    );
    denied(
        &scene.page("node.unknown", &GoalPagePart::Records, None, None),
        "instance_unknown",
        "operation",
    );
    scene.close();
}
