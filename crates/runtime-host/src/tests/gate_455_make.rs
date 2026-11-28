// SPDX-License-Identifier: AGPL-3.0-only

// #455 MAKE (0.12.104; register §1.558; the coordinator's rulings of 11-29 in its work order, on
// the read-only study of the same day; contracts/standard-pack-goal-tables.md, "What is
// computed"), the host's half. Taking the needs apart, the pool of an item with a recipe whose
// holding is known and below its need asks, beside what the holding covers, the made part: what
// the items in hand make now, `min(need − held, k·makes)`, `k` the least over the recipe's `from`
// and `cost` items of ⌊holding ÷ what one making takes⌋, at most the makings the need asks
// (`PoolNeedKind::Making`), with the pools of the recipe's `from` items:
// - the study's numbers: 8 `low` and 1000 `coin` in hand make 2 of the 6 `top` not covered; the
//   inputs' holdings bound the set's freshness (m1);
// - inputs that pay for no making leave the set byte for byte as before, its identity included
//   (m2); so does an input whose holding is not known or unread (m3), or that names no pool (m5);
// - the made part is bounded by the part not covered and by what the inputs pay for (m4);
// - two levels of recipes, each taken with the holdings in hand (m6);
// - the identity hashes the kind and the made part, and not the pools of the `from` items (m7).
//
// The tables, keys, items and pools are made up; times are fixed Unix milliseconds, and nothing
// sleeps.

use super::*;
use crate::goal_needs::{NeedsSources, RecordsRoot, instance_needs};
use crate::goal_records::{GOAL_RECORDS_SCHEMA_VERSION, StoredGoalRecords};
use actingcommand_contract::{
    FactScalar, RuntimeChannel, StandardPackGoalTables, parse_standard_pack_goal_tables,
};
use actingcommand_policy::{
    InstanceRecordNeeds, NeedHolding, PoolNeed, PoolNeedKind, ProgressListUnknown, RecordNeedsNote,
};
use serde_json::{Value, json};

const ALIAS: &str = POLICY_INSTANCE_ALIAS;
const NOW: u64 = POLICY_NOW_UNIX_MS;
/// When the fixture's roster is read: ten seconds before the fixture's time.
const READ_AT: u64 = POLICY_NOW_UNIX_MS - 10_000;
/// How long the roster stays fresh.
const FRESH_MS: u64 = 60_000;
/// The table's `held_by` list, which gives `grade`.
const ROSTER: &str = "inventory.list.fixture_roster";
/// The records entry, and the table it names.
const ENTRY: &str = "raise";
const TABLE: &str = "units";
/// The game the fixture's tables files are written for.
const TABLES_GAME: &str = "neutral";
const SHA: &str = "fixture-tables-make";

/// What the computation reads besides the records and the root: lists by key (fresh until their
/// expiry at the fixture's time) and holdings by pool (none: not known).
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

fn text(value: &str) -> FactScalar {
    FactScalar::String(value.to_owned())
}

/// The roster with `unit_a` at grade 0, read at [`READ_AT`], fresh for [`FRESH_MS`], at
/// confidence 900.
fn roster() -> FactRecord {
    let mut fact = stored_fact(
        FactScope::Instance {
            instance_id: ALIAS.to_owned(),
        },
        ROSTER,
        ContractFactValue::RecordList(vec![BTreeMap::from([
            ("unit".to_owned(), text("unit_a")),
            ("grade".to_owned(), FactScalar::Integer(0)),
        ])]),
        "snapshot:make:roster",
        Vec::new(),
    );
    fact.observed_at_unix_ms = READ_AT;
    fact.expires_at_unix_ms = Some(READ_AT + FRESH_MS);
    fact.confidence_milli = 900;
    fact
}

/// The instance's stored records: one, raising `unit_a` to grade 1, set at the fixture's time on
/// the CLI, read by the goal records' own reader.
fn stored() -> StoredGoalRecords {
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
            ("to".to_owned(), text(r#"{"grade":[1]}"#)),
            (
                "set_unix_ms".to_owned(),
                FactScalar::Integer(i64::try_from(NOW).expect("the fixture's time")),
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

/// The tables of one table whose key `unit_a` takes one grade step, `step`, over `items`.
fn table_of(items: Value, step: Value) -> Value {
    json!({
        "schema_version": "actingcommand.goal-tables.v1",
        "game": TABLES_GAME,
        "items": items,
        "tables": [{
            "name": TABLE,
            "progress": [{"fact_key": ROSTER, "key_column": "unit", "held_by": true, "columns": ["grade"]}],
            "columns": [{"name": "grade", "read": ["grade"], "per": "class"}],
            "classes": {"all": {"grade": {"positions": [[0], [1]], "steps": [step]}}},
            "keys": {"unit_a": {"class": "all"}}
        }]
    })
}

/// RR3's tables: `top` is made `makes` at a time for 4 `low` and 300 `coin`; the step needs `need`
/// of `top` and 2 `seal`, an item of no pool.
fn recipe_tables(makes: u64, need: u64) -> Value {
    table_of(
        json!({
            "top": {"pool": "pool.top", "recipe": {"makes": makes, "from": {"low": 4}, "cost": {"coin": 300}}},
            "low": {"pool": "pool.low"},
            "coin": {"pool": "pool.coin"},
            "seal": {}
        }),
        json!({"top": need, "seal": 2}),
    )
}

/// The needs of the stored record over `document`, reading the roster and `holdings`.
fn needs_of(document: &Value, holdings: &[(&str, NeedHolding)]) -> InstanceRecordNeeds {
    let sources = Sources {
        lists: BTreeMap::from([(ROSTER.to_owned(), roster())]),
        holdings: holdings
            .iter()
            .map(|(pool, holding)| ((*pool).to_owned(), *holding))
            .collect(),
    };
    let root = RecordsRoot {
        entries: BTreeMap::from([(ENTRY.to_owned(), TABLE.to_owned())]),
        tables: Some((Arc::new(tables(document)), SHA.to_owned())),
    };
    instance_needs(&stored(), NOW, Some(&root), &sources)
        .expect("an instance with records has needs")
}

/// A holding of `held`, fresh until `fresh_until_unix_ms`.
fn known(held: u64, fresh_until_unix_ms: Option<u64>) -> NeedHolding {
    NeedHolding::Known {
        held,
        fresh_until_unix_ms,
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

/// The note of `seal`, which RR3's tables need 2 of and which weighs nothing.
fn seal() -> RecordNeedsNote {
    RecordNeedsNote::ItemUnweighed {
        item: "seal".to_owned(),
        need: 2,
    }
}

/// The set RR3 computes on its tables (its `rr3_p1`): `top` held 4, its inputs' holdings not
/// known.
fn rr3_set() -> InstanceRecordNeeds {
    needs_of(&recipe_tables(1, 10), &[("pool.top", known(4, None))])
}

// m1, the study's numbers (ruling 1). The record needs 10 `top`, held 4: 6 are not covered, 6
// makings of 4 `low` and 300 `coin`. 8 `low` and 1000 `coin` in hand pay for min(6, ⌊8/4⌋,
// ⌊1000/300⌋) = 2 makings, so `top`'s pool asks 4 + 2 (`Making { made: 2 }`), made from `low`'s pool;
// `low` and `coin` keep the 24 and 1800 the taking apart gives them. The set's freshness is the
// earliest of the roster's and the two inputs' holdings, whichever input's ends first.
#[test]
fn make_m1_the_items_in_hand_make_part_of_what_is_not_covered() {
    for (low_until, coin_until, earliest) in [
        (READ_AT + 30_000, READ_AT + 40_000, READ_AT + 30_000),
        (READ_AT + 50_000, READ_AT + 20_000, READ_AT + 20_000),
    ] {
        let needs = needs_of(
            &recipe_tables(1, 10),
            &[
                ("pool.top", known(4, None)),
                ("pool.low", known(8, Some(low_until))),
                ("pool.coin", known(1_000, Some(coin_until))),
            ],
        );
        assert_eq!(
            needs.pools(),
            &pools(&[
                ("pool.coin", 1_800, PoolNeedKind::Lands),
                ("pool.low", 24, PoolNeedKind::Lands),
                ("pool.top", 6, PoolNeedKind::Making { made: 2 }),
            ])
        );
        assert_eq!(needs.making_from("pool.top"), ["pool.low"]);
        assert_eq!(needs.notes(), [seal()]);
        assert_eq!(needs.fresh_until_unix_ms(), Some(earliest));
        assert_eq!(needs.tables_sha256(), Some(SHA));
    }
}

// m2, inputs that pay for no making (ruling 1: `made` 0). 3 `low` (⌊3/4⌋ = 0), or 200 `coin`
// (⌊200/300⌋ = 0): `top`'s pool asks its holding's 4 (`Covered`), and the set is byte for byte RR3's,
// its identity included: the inputs' holdings, though read, bound no freshness.
#[test]
fn make_m2_items_in_hand_that_pay_for_no_making_leave_the_set_as_before() {
    let before = rr3_set();
    assert_eq!(
        before.pools(),
        &pools(&[
            ("pool.coin", 1_800, PoolNeedKind::Lands),
            ("pool.low", 24, PoolNeedKind::Lands),
            ("pool.top", 4, PoolNeedKind::Covered),
        ])
    );
    assert_eq!(before.fresh_until_unix_ms(), Some(READ_AT + FRESH_MS));
    for (case, low, coin) in [("low short", 3, 1_000), ("coin short", 8, 200)] {
        let short = needs_of(
            &recipe_tables(1, 10),
            &[
                ("pool.top", known(4, None)),
                ("pool.low", known(low, Some(READ_AT + 20_000))),
                ("pool.coin", known(coin, Some(READ_AT + 20_000))),
            ],
        );
        assert_eq!(short, before, "{case}");
        assert_eq!(short.identity(), before.identity(), "{case}");
        assert!(short.making_from("pool.top").is_empty(), "{case}");
    }
}

// m3, an input whose holding is not known, or unread (ruling 1: `k` 0). `coin` not known, `coin`
// unread (RR3c), or `low` unread, the other input in hand and fresh for a shorter time than the
// roster: the set is RR3's, `Covered` 4, and no input bounds its freshness.
#[test]
fn make_m3_an_input_not_known_or_unread_makes_nothing() {
    let before = rr3_set();
    let early = Some(READ_AT + 20_000);
    for (case, low, coin) in [
        (
            "coin not known",
            known(8, early),
            NeedHolding::Unknown(None),
        ),
        ("coin unread", known(8, early), NeedHolding::Unread),
        ("low unread", NeedHolding::Unread, known(1_000, early)),
    ] {
        let needs = needs_of(
            &recipe_tables(1, 10),
            &[
                ("pool.top", known(4, None)),
                ("pool.low", low),
                ("pool.coin", coin),
            ],
        );
        assert_eq!(needs, before, "{case}");
        assert_eq!(
            needs.fresh_until_unix_ms(),
            Some(READ_AT + FRESH_MS),
            "{case}"
        );
    }
}

// m4, the bounds (ruling 1). `top` made 3 at a time, needed 10 and held 6: 4 are not covered, ⌈4/3⌉
// = 2 makings (8 `low`, 600 `coin`). 8 `low` and 1000 `coin` pay for both: min(4, 2·3) = 4, the part
// not covered. 4 `low` pay for one: min(4, 1·3) = 3.
#[test]
fn make_m4_the_made_part_is_bounded_by_the_part_not_covered_and_by_the_inputs() {
    for (low, made) in [(8, 4), (4, 3)] {
        let needs = needs_of(
            &recipe_tables(3, 10),
            &[
                ("pool.top", known(6, None)),
                ("pool.low", known(low, None)),
                ("pool.coin", known(1_000, None)),
            ],
        );
        assert_eq!(
            needs.pools(),
            &pools(&[
                ("pool.coin", 600, PoolNeedKind::Lands),
                ("pool.low", 8, PoolNeedKind::Lands),
                ("pool.top", 6 + made, PoolNeedKind::Making { made }),
            ]),
            "low {low}"
        );
    }
}

// m5, an input item that names no pool (ruling 1: `k` 0). `top` made from 4 `low` and 1 `bare`, an
// item of no pool, and 300 `coin`, every pool's holding in hand: `top` asks its holding's 4
// (`Covered`), and `bare`'s 6 are noted and weigh nothing, as before.
#[test]
fn make_m5_an_input_without_a_pool_makes_nothing() {
    let document = table_of(
        json!({
            "top": {"pool": "pool.top", "recipe": {"makes": 1, "from": {"low": 4, "bare": 1}, "cost": {"coin": 300}}},
            "low": {"pool": "pool.low"},
            "bare": {},
            "coin": {"pool": "pool.coin"}
        }),
        json!({"top": 10}),
    );
    let needs = needs_of(
        &document,
        &[
            ("pool.top", known(4, None)),
            ("pool.low", known(8, None)),
            ("pool.coin", known(1_000, None)),
        ],
    );
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
            item: "bare".to_owned(),
            need: 6,
        }]
    );
    assert!(needs.making_from("pool.top").is_empty());
}

// m6, two levels (ruling 1). `p` is made 1 for 2 `q`, `q` 1 for 3 `l`; the record needs 5 `p`, held
// 2, so 3 makings and 6 `q`. With no `q` in hand `p` makes nothing (`Covered` 2), and `q`, held 0,
// makes min(6, ⌊9/3⌋) = 3 of its 6 from 9 `l` (`Making { made: 3 }`); `l` needs 18. With 4 `q` in
// hand `p` makes min(3, ⌊4/2⌋) = 2 (asks 4) and `q` makes the 2 its holding leaves (asks 6), each
// holding taken whole; `l` needs 6.
#[test]
fn make_m6_each_level_makes_what_its_items_in_hand_pay_for() {
    let document = table_of(
        json!({
            "p": {"pool": "pool.p", "recipe": {"makes": 1, "from": {"q": 2}}},
            "q": {"pool": "pool.q", "recipe": {"makes": 1, "from": {"l": 3}}},
            "l": {"pool": "pool.l"}
        }),
        json!({"p": 5}),
    );
    let without_q = needs_of(
        &document,
        &[
            ("pool.p", known(2, None)),
            ("pool.q", known(0, None)),
            ("pool.l", known(9, None)),
        ],
    );
    assert_eq!(
        without_q.pools(),
        &pools(&[
            ("pool.l", 18, PoolNeedKind::Lands),
            ("pool.p", 2, PoolNeedKind::Covered),
            ("pool.q", 3, PoolNeedKind::Making { made: 3 }),
        ])
    );
    assert!(without_q.making_from("pool.p").is_empty());
    assert_eq!(without_q.making_from("pool.q"), ["pool.l"]);

    let with_q = needs_of(
        &document,
        &[
            ("pool.p", known(2, None)),
            ("pool.q", known(4, None)),
            ("pool.l", known(9, None)),
        ],
    );
    assert_eq!(
        with_q.pools(),
        &pools(&[
            ("pool.l", 6, PoolNeedKind::Lands),
            ("pool.p", 4, PoolNeedKind::Making { made: 2 }),
            ("pool.q", 6, PoolNeedKind::Making { made: 2 }),
        ])
    );
    assert_eq!(with_q.making_from("pool.p"), ["pool.q"]);
    assert_eq!(with_q.making_from("pool.q"), ["pool.l"]);
}

// m7, the identity (ruling 1, the study's item 6). A set with a pool of the kind `Making` hashes
// apart from RR3's; on one pool of need 6 the kinds `Making` of 2 and of 3, `Covered` and `Lands`
// give four identities; the pools of the `from` items are not hashed: the same set with them has
// the identity of the set without them, though the two sets differ.
#[test]
fn make_m7_the_identity_hashes_the_kind_and_the_made_part() {
    let making = needs_of(
        &recipe_tables(1, 10),
        &[
            ("pool.top", known(4, None)),
            ("pool.low", known(8, None)),
            ("pool.coin", known(1_000, None)),
        ],
    );
    assert_ne!(making.identity(), rr3_set().identity());
    let set_of = |kind| {
        InstanceRecordNeeds::new(
            Some(SHA.to_owned()),
            NOW,
            pools(&[("pool.top", 6, kind)]),
            Vec::new(),
            None,
        )
    };
    let identities = [
        PoolNeedKind::Making { made: 2 },
        PoolNeedKind::Making { made: 3 },
        PoolNeedKind::Covered,
        PoolNeedKind::Lands,
    ]
    .map(|kind| set_of(kind).identity().to_owned());
    assert_eq!(
        identities.iter().collect::<BTreeSet<_>>().len(),
        identities.len(),
        "{identities:?}"
    );
    let plain = set_of(PoolNeedKind::Making { made: 2 });
    let named = set_of(PoolNeedKind::Making { made: 2 }).with_making_from(BTreeMap::from([(
        "pool.top".to_owned(),
        vec!["pool.low".to_owned()],
    )]));
    assert_eq!(named.identity(), plain.identity());
    assert_ne!(named, plain);
    assert_eq!(named.making_from("pool.top"), ["pool.low"]);
}
