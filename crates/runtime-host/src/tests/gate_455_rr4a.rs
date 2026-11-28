// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #455 RR4a (0.12.100; register §1.546; contracts/standard-pack-goal-tables.md, "Names for
// people" and "Who reads it"), the host's half of test 4: the Runtime computes nothing from a name.
// A tables file that writes `names` and the same file without it read to the same tables, and the
// needs computed from one records entry over either are the same set but for the file's sha256,
// which the set carries and hashes into its identity. The workbench's keeping of the names (test
// 6) extends `gate_455_rr1`'s h1. The tables, keys, items and pools are made up; times are fixed
// Unix milliseconds, and nothing sleeps.

use super::*;
use crate::goal_needs::{NeedsSources, RecordsRoot, instance_needs};
use crate::goal_records::{GOAL_RECORDS_SCHEMA_VERSION, StoredGoalRecords};
use actingcommand_contract::{
    FactScalar, GoalTableNames, RuntimeChannel, StandardPackGoalTables,
    parse_standard_pack_goal_tables,
};
use actingcommand_policy::{NeedHolding, ProgressListUnknown};
use serde_json::{Value, json};

const ALIAS: &str = POLICY_INSTANCE_ALIAS;
/// When the fixture's list is read: ten seconds before the fixture's time.
const READ_AT: u64 = POLICY_NOW_UNIX_MS - 10_000;
/// How long a list read stays fresh.
const FRESH_MS: u64 = 60_000;
/// The table's `held_by` list, which gives `grade`.
const ROSTER: &str = "inventory.list.fixture_roster";
/// The records entry, and the table it names.
const ENTRY: &str = "raise";
const TABLE: &str = "units";
/// The game the fixture's tables files are written for.
const TABLES_GAME: &str = "neutral";
/// The sha256 the workbench would keep for each of the two files (made up: the computation only
/// carries it).
const PLAIN_SHA: &str = "fixture-tables-plain";
const NAMED_SHA: &str = "fixture-tables-named";

/// What the computation reads besides the records and the root: lists by key (fresh until their
/// expiry) and one holding for every pool asked.
struct Sources {
    lists: BTreeMap<String, FactRecord>,
    holding: NeedHolding,
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
        Ok(expires_at_unix_ms)
    }

    fn holding(&self, _pool_id: &str) -> NeedHolding {
        self.holding
    }
}

fn text(value: &str) -> FactScalar {
    FactScalar::String(value.to_owned())
}

/// The roster with `unit_a` at `grade`, read at [`READ_AT`], fresh for [`FRESH_MS`], at
/// confidence 900.
fn roster(grade: i64) -> FactRecord {
    let rows = vec![BTreeMap::from([
        ("unit".to_owned(), text("unit_a")),
        ("grade".to_owned(), FactScalar::Integer(grade)),
    ])];
    let mut fact = stored_fact(
        FactScope::Instance {
            instance_id: ALIAS.to_owned(),
        },
        ROSTER,
        ContractFactValue::RecordList(rows),
        "snapshot:rr4a:roster",
        Vec::new(),
    );
    fact.observed_at_unix_ms = READ_AT;
    fact.expires_at_unix_ms = Some(READ_AT + FRESH_MS);
    fact.confidence_milli = 900;
    fact
}

/// The instance's stored records: the header and one record of `unit_a` to the canonical `to`,
/// set at the fixture's time on the CLI, read by the goal records' own reader.
fn stored(to: &str) -> StoredGoalRecords {
    let rows = vec![
        BTreeMap::from([
            ("row".to_owned(), text("header")),
            (
                "schema_version".to_owned(),
                text(GOAL_RECORDS_SCHEMA_VERSION),
            ),
            ("instance".to_owned(), text(ALIAS)),
        ]),
        BTreeMap::from([
            ("row".to_owned(), text("record")),
            ("entry".to_owned(), text(ENTRY)),
            ("key".to_owned(), text("unit_a")),
            ("to".to_owned(), text(to)),
            (
                "set_unix_ms".to_owned(),
                FactScalar::Integer(i64::try_from(POLICY_NOW_UNIX_MS).expect("the fixture's time")),
            ),
            ("channel".to_owned(), text(RuntimeChannel::Cli.as_str())),
        ]),
    ];
    let read = StoredGoalRecords::read(Some(rows.as_slice()), ALIAS);
    assert!(
        matches!(&read, StoredGoalRecords::Read(taken) if taken.len() == 1),
        "the fixture's records read: {read:?}"
    );
    read
}

fn tables(document: &Value) -> StandardPackGoalTables {
    parse_standard_pack_goal_tables(document.to_string().as_bytes(), TABLES_GAME)
        .unwrap_or_else(|issues| panic!("the fixture's tables read: {issues:?}"))
}

/// RR3's tables of taking apart: `top` is made 1 for 4 `low` and 300 `coin`; a key's one grade
/// step needs 10 `top`.
fn recipe_tables() -> Value {
    json!({
        "schema_version": "actingcommand.goal-tables.v1",
        "game": TABLES_GAME,
        "items": {
            "top": {"pool": "pool.top", "recipe": {"makes": 1, "from": {"low": 4}, "cost": {"coin": 300}}},
            "low": {"pool": "pool.low"},
            "coin": {"pool": "pool.coin"}
        },
        "tables": [{
            "name": TABLE,
            "progress": [{"fact_key": ROSTER, "key_column": "unit", "held_by": true, "columns": ["grade"]}],
            "columns": [{"name": "grade", "read": ["grade"], "per": "class"}],
            "classes": {"all": {"grade": {"positions": [[0], [1]], "steps": [{"top": 10}]}}},
            "keys": {"unit_a": {"class": "all"}}
        }]
    })
}

// Test 4, the host's half. The same file with `names` (every item, the table's key, column and
// class named, the column's positions said) and without it read to the same game, items and
// tables, the second's names none. A record that raises `unit_a` one grade, its holding of `top`
// known at 4, over either: the same pools (4 `top` covered, 24 `low` and 1800 `coin` landing,
// RR3's numbers), notes, records' time and freshness; the set carries each file's own sha256, so
// the two identities differ; computed over the named tables with the plain file's sha256, the set
// is the plain one, identity and all.
#[test]
fn rr4a_p1_a_file_with_names_and_the_same_file_without_give_the_same_needs_but_their_sha256() {
    let plain = tables(&recipe_tables());
    let mut document = recipe_tables();
    document["names"] = json!({
        "items": {"top": {"en": "Top part"}, "low": {"en": "Low part"}, "coin": {"en": "Coin"}},
        "tables": {"units": {
            "keys": {"unit_a": {"en": "First unit"}},
            "columns": {"grade": {"en": "Grade"}},
            "classes": {"all": {"en": "Every unit"}},
            "positions": {"grade": [{"prefix": [], "text": {"en": "Grade {0}"}}]}
        }}
    });
    let named = tables(&document);
    assert_eq!(plain.names, GoalTableNames::default());
    assert_ne!(named.names, GoalTableNames::default());
    assert_eq!(named.game, plain.game);
    assert_eq!(named.items, plain.items);
    assert_eq!(named.tables, plain.tables);

    let records = stored(r#"{"grade":[1]}"#);
    let sources = Sources {
        lists: BTreeMap::from([(ROSTER.to_owned(), roster(0))]),
        holding: NeedHolding::Known {
            held: 4,
            fresh_until_unix_ms: None,
        },
    };
    let needs_over = |parsed: &StandardPackGoalTables, sha: &str| {
        let root = RecordsRoot {
            entries: BTreeMap::from([(ENTRY.to_owned(), TABLE.to_owned())]),
            tables: Some((Arc::new(parsed.clone()), sha.to_owned())),
        };
        instance_needs(&records, POLICY_NOW_UNIX_MS, Some(&root), &sources)
            .expect("an instance with records has needs")
    };
    let without = needs_over(&plain, PLAIN_SHA);
    let with_names = needs_over(&named, NAMED_SHA);
    assert_eq!(without.pools().len(), 3, "{:?}", without.pools());
    assert_eq!(with_names.pools(), without.pools());
    assert_eq!(with_names.notes(), without.notes());
    assert_eq!(
        with_names.records_observed_at_unix_ms(),
        without.records_observed_at_unix_ms()
    );
    assert_eq!(
        with_names.fresh_until_unix_ms(),
        without.fresh_until_unix_ms()
    );
    assert_eq!(with_names.tables_sha256(), Some(NAMED_SHA));
    assert_eq!(without.tables_sha256(), Some(PLAIN_SHA));
    assert_ne!(with_names.identity(), without.identity());
    assert_eq!(needs_over(&named, PLAIN_SHA), without);
}
