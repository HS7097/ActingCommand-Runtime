// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #455 RR2 (0.12.94; register §1.537; contracts/instance-fact-store.md, "Goal records
// facts"; contracts/standard-pack-goals.md, "The records entry"; contracts/runtime-interface.md,
// "Goal request kinds"; coordinator work order rd-1123/brief-455-rr2.md): a records entry's value
// is stored. The goal edit of a records entry the root in force applies writes the instance's
// records into the instance fact `session.goal_records`, the key's one writer; a fact reader reads
// them back; the goal page keeps one version over that record and `session.resource_targets`;
// nothing is computed from the records.
//
// The first test runs the edit's checks and the record's composition as pure functions, on a page
// whose `goals.json` holds three records entries (one on a table of the root's `goal-tables.json`,
// one on a table the file does not have, one on a table whose keys have long column and sub names).
// The others run the host, as `gate_503_goal1` does, in composer mode over one game root on disk
// with two instances of its game, a manual Runtime clock and fake devices; its `goals.json` holds a
// `resource` entry and a `records` entry on the table `heroes` of its `goal-tables.json`. No
// package runs; no clock is read and nothing sleeps. Every name and amount is made up.

use super::gate_442_rem1::write_package;
use super::gate_rl1b::{
    GAME_A, POOL_A, SERVER_A, bundle, pool, profiles, resources, rule, write_json,
};
use super::*;
use crate::goal_records::{GivenRecord, RecordsChange, StoredGoalRecords, compose};
use crate::goals::{EditRequest, GoalChange, GoalPage, GoalRefusal, check_goal_edit};
use actingcommand_contract::{
    DeclaredInitiator, FactPayload, FactScalar as ContractFactScalar, GOAL_RECORDS_FACT_KEY,
    GoalEdit, GoalEdited, GoalProblem, GoalView, Initiator, RESOURCE_TARGETS_FACT_KEY,
    RuntimeChannel, StandardPackGoalTables, parse_standard_pack_goal_tables,
    parse_standard_pack_goals,
};
use actingcommand_policy::{StoredGoalRecord, TargetSetter, VariableSetBy};
use serde_json::{Value, json};

const INSTANCE_A: &str = POLICY_INSTANCE_ALIAS;
const INSTANCE_B: &str = POLICY_INSTANCE_ALIAS_B;
const T0: u64 = POLICY_NOW_UNIX_MS;
/// The root's one rule and its package.
const TASK: &str = "fixture.records";
/// The `resource` entry of the root's `goals.json`, on [`POOL_A`].
const STOCK: &str = "stock";
/// The `records` entry of the root's `goals.json`, on [`TABLE`], at most two records.
const RECORDS: &str = "raise";
/// The table of the root's `goal-tables.json` the records entry names.
const TABLE: &str = "heroes";
const VIEWED: &str = "request_goal_view_completed";
const EDITED: &str = "request_edit_goal_completed";
const INVALID: &str = "goal_value_invalid";
const CONFLICT: &str = "fact_observation_version_conflict";
const FORMAL: &str = "goal_records_formal_entry_required";
/// The default `standard_packs.check_interval_ms`: a check reads the roots again after it.
const CHECK_INTERVAL_MS: u64 = 60_000;
/// The canonical text of [`raise_a`]'s `to`: compact, the members in byte order.
const TO_A: &str = r#"{"grade":[2],"mastery":{"1":[1]},"skill":[3]}"#;

/// `hero_a`: of the class `common` (whose `grade` ladder is `[0], [1], [2]`), with its own `skill`
/// ladder `[1], [2], [3]` and a `mastery` ladder `[0], [1]` for its sub `1`.
fn hero_a() -> Value {
    json!({"class": "common",
           "skill": {"positions": [[1], [2], [3]], "steps": [{"coin": 5}, {"coin": 6}]},
           "mastery": {"1": {"positions": [[0], [1]], "steps": [{"coin": 7}]}}})
}

/// The table `heroes`: the columns `grade` (per class), `skill` (per key) and `mastery` (per sub,
/// one sub `1`), the class `common` and the keys `hero_a` and, with `with_b`, `hero_b` (of the
/// class alone: no `skill`, no `mastery`).
fn heroes(with_b: bool) -> Value {
    let mut keys = serde_json::Map::new();
    keys.insert("hero_a".to_owned(), hero_a());
    if with_b {
        keys.insert("hero_b".to_owned(), json!({"class": "common"}));
    }
    json!({
        "name": TABLE,
        "progress": [{"fact_key": "inventory.list.fixture_heroes", "key_column": "hero",
                      "held_by": true, "columns": ["grade", "skill", "mastery_1"]}],
        "columns": [{"name": "grade", "read": ["grade"], "per": "class"},
                    {"name": "skill", "read": ["skill"], "per": "key"},
                    {"name": "mastery", "per": "sub", "subs": {"1": ["mastery_1"]}}],
        "classes": {"common": {"grade": {"positions": [[0], [1], [2]],
                                         "steps": [{"coin": 10}, {"coin": 20}]}}},
        "keys": keys
    })
}

/// The name of the column `letter` of [`long_table`]: 60 bytes.
fn long_column(letter: char) -> String {
    format!("{}_{letter}", "x".repeat(58))
}

/// The name of the sub `index` of [`long_table`]'s columns: 59 bytes.
fn long_sub(index: usize) -> String {
    format!("s{index}_{}", "y".repeat(56))
}

/// The table `long`: a class column `g`, and two columns of per sub, each of eight subs, all of
/// long names, which its one key `k` has every ladder of; a `to` that names every sub is longer
/// than 1024 bytes.
fn long_table() -> Value {
    let ladder = json!({"positions": [[0], [1]], "steps": [{"coin": 1}]});
    let mut columns = vec![json!({"name": "g", "read": ["g"], "per": "class"})];
    let mut key = serde_json::Map::new();
    key.insert("class".to_owned(), json!("c"));
    let mut reading = vec![json!("g")];
    for letter in ['a', 'b'] {
        let mut subs = serde_json::Map::new();
        let mut ladders = serde_json::Map::new();
        for index in 0..8 {
            let read = format!("r{letter}{index}");
            subs.insert(long_sub(index), json!([read]));
            ladders.insert(long_sub(index), ladder.clone());
            reading.push(Value::String(read));
        }
        columns.push(json!({"name": long_column(letter), "per": "sub", "subs": subs}));
        key.insert(long_column(letter), Value::Object(ladders));
    }
    json!({
        "name": "long",
        "progress": [{"fact_key": "inventory.list.fixture_long", "key_column": "id",
                      "columns": reading}],
        "columns": columns,
        "classes": {"c": {"g": ladder}},
        "keys": {"k": key}
    })
}

/// The root's `goal-tables.json` of `tables`.
fn tables_file(tables: Vec<Value>) -> Value {
    json!({
        "schema_version": "actingcommand.goal-tables.v1",
        "game": GAME_A,
        "items": {"coin": {"pool": POOL_A}},
        "tables": tables
    })
}

/// The root's `goals.json` (version 3): the `resource` entry [`STOCK`] and, with `records`, the
/// records entry [`RECORDS`] on [`TABLE`], at most two records.
fn goals(records: bool) -> Value {
    let mut entries = vec![
        json!({"name": STOCK, "type": "resource", "governs": {"pool": POOL_A},
                                  "title": {"en": "Stock"}}),
    ];
    if records {
        entries.push(json!({"name": RECORDS, "type": "records", "table": TABLE,
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

/// A value of [`RECORDS`]: `hero_a` to grade `[2]`, skill `[3]` and mastery sub `1` at `[1]`, its
/// columns written out of their byte order (its canonical text is [`TO_A`]).
fn raise_a() -> Value {
    json!([{"key": "hero_a", "to": {"skill": [3], "grade": [2], "mastery": {"1": [1]}}}])
}

// ---- The pure part: the checks and the composition. ----

/// The pure test's page: a `goals.json` of three records entries, `raise` on `heroes` (at most
/// two), `spare` on a table the root's file does not have, and `wide` on `long` (at most one).
fn pure_page() -> GoalPage {
    let document = json!({"schema_version": "actingcommand.goals.v3", "game": GAME_A, "entries": [
        {"name": RECORDS, "type": "records", "table": TABLE, "max_records": 2,
         "title": {"en": "Whom to raise"}},
        {"name": "spare", "type": "records", "table": "missing", "max_records": 1,
         "title": {"en": "Spare"}},
        {"name": "wide", "type": "records", "table": "long", "max_records": 1,
         "title": {"en": "Wide"}}
    ]});
    let bytes = serde_json::to_vec(&document).expect("the goals' bytes");
    let declaration = parse_standard_pack_goals(&bytes, GAME_A).expect("the goals read");
    GoalPage {
        game_id: GAME_A.to_owned(),
        unchecked: None,
        declaration: Some(declaration),
        packages: Vec::new(),
        targetable: Vec::new(),
        capacities: BTreeMap::new(),
        not_applying: BTreeMap::new(),
        root: BTreeMap::new(),
    }
}

/// The pure test's tables: `heroes` with both keys, and `long`.
fn pure_tables() -> StandardPackGoalTables {
    let bytes = serde_json::to_vec(&tables_file(vec![heroes(true), long_table()]))
        .expect("the tables' bytes");
    parse_standard_pack_goal_tables(&bytes, GAME_A).expect("the tables read")
}

fn writer() -> VariableSetBy {
    VariableSetBy {
        set_unix_ms: T0,
        channel: RuntimeChannel::Lab,
    }
}

fn setter() -> TargetSetter {
    TargetSetter {
        set_unix_ms: T0,
        channel: RuntimeChannel::Lab,
        initiator: Some(Initiator::LabCli),
    }
}

/// What the checks answer for `request`, on `records` stored.
fn checked(
    page: &GoalPage,
    tables: Option<&StandardPackGoalTables>,
    records: &StoredGoalRecords,
    request: EditRequest<'_>,
) -> Result<GoalChange, GoalRefusal> {
    check_goal_edit(
        page,
        tables,
        &StoredGoalRecord::Empty,
        records,
        request,
        writer(),
    )
}

/// The word, the pointer and the range of a refused `request`.
#[track_caller]
fn refused_with(
    page: &GoalPage,
    tables: Option<&StandardPackGoalTables>,
    records: &StoredGoalRecords,
    request: EditRequest<'_>,
) -> (GoalProblem, Option<String>, Option<(i64, i64)>) {
    let refusal = checked(page, tables, records, request).expect_err("the request is refused");
    (refusal.problem, refusal.pointer, refusal.range)
}

// Rulings 3 and 4, and the form. On the page above with the root's tables: a set of `raise`
// whose records are well formed is taken as one change of the goal records record, each `to` in
// its canonical text (its columns and subs in byte order). Each word at its cause, one case a
// row, with its pointer and its range: the value no array, a record no object, a member a record
// does not have, a key that is no text or is missing, an empty `to`, a `to` no object or missing
// (`wrong_type`); a key the table does not hold (`not_an_option`); a key twice (`key_repeated`, at
// the later); three records over the entry's two (`count_out_of_range`, 0 to 2); a column the
// table does not have, a column the key has no ladder for, a position off the ladder or of
// another width, a sub the key does not have, a sub's position off its ladder (`out_of_range`, at
// the column or the sub); a position no array or not of whole numbers, a sub column's value no
// object or naming no sub (`wrong_type`); a `to` whose canonical text passes 1024 bytes
// (`value_too_large`, at the `to`, 0 to 1024). A records entry the root does not apply (its table
// no table of the file; or the root holds no accepted file) answers a set as an unknown name does
// (`name_unknown`). A clear of `raise` with nothing stored is `value_not_set`, of `spare`
// `name_unknown`; a clear of a name under which records are stored removes them, declared or not.
// The instance's 128 records: 128 of an entry the page does not declare are composed; a set of
// `raise` over them passes the checks and is refused by the record, `record_out_of_limits` (0 to
// 128), while a set of none, and the same 128 replaced, are composed.
#[test]
fn rr2_p1_each_word_of_a_records_value_at_its_cause_and_the_instance_s_bound() {
    let page = pure_page();
    let tables = pure_tables();
    let none = StoredGoalRecords::Absent;
    let set = |value: &Value| value.to_string();

    let both = set(&json!([
        {"key": "hero_a", "to": {"skill": [3], "grade": [2], "mastery": {"1": [1]}}},
        {"key": "hero_b", "to": {"grade": [1]}}
    ]));
    assert_eq!(
        checked(
            &page,
            Some(&tables),
            &none,
            EditRequest::Set {
                name: RECORDS,
                value_json: &both
            }
        ),
        Ok(GoalChange::Records(RecordsChange::Set {
            entry: RECORDS.to_owned(),
            records: vec![
                GivenRecord {
                    key: "hero_a".to_owned(),
                    to: TO_A.to_owned(),
                },
                GivenRecord {
                    key: "hero_b".to_owned(),
                    to: r#"{"grade":[1]}"#.to_owned(),
                },
            ],
        }))
    );

    use GoalProblem as P;
    let cases: Vec<(Value, P, Option<&str>, Option<(i64, i64)>)> = vec![
        (json!({"key": "hero_a"}), P::WrongType, None, None),
        (json!([1]), P::WrongType, Some("/0"), None),
        (
            json!([{"key": "hero_a", "to": {"grade": [1]}, "why": 1}]),
            P::WrongType,
            Some("/0/why"),
            None,
        ),
        (
            json!([{"key": 1, "to": {"grade": [1]}}]),
            P::WrongType,
            Some("/0/key"),
            None,
        ),
        (
            json!([{"to": {"grade": [1]}}]),
            P::WrongType,
            Some("/0/key"),
            None,
        ),
        (
            json!([{"key": "hero_z", "to": {"grade": [1]}}]),
            P::NotAnOption,
            Some("/0/key"),
            None,
        ),
        (
            json!([{"key": "hero_a", "to": {"grade": [1]}},
                   {"key": "hero_a", "to": {"grade": [2]}}]),
            P::KeyRepeated,
            Some("/1/key"),
            None,
        ),
        (
            json!([{"key": "hero_a", "to": {"grade": [1]}},
                   {"key": "hero_b", "to": {"grade": [1]}},
                   {"key": "hero_c", "to": {"grade": [1]}}]),
            P::CountOutOfRange,
            None,
            Some((0, 2)),
        ),
        (
            json!([{"key": "hero_a"}]),
            P::WrongType,
            Some("/0/to"),
            None,
        ),
        (
            json!([{"key": "hero_a", "to": {}}]),
            P::WrongType,
            Some("/0/to"),
            None,
        ),
        (
            json!([{"key": "hero_a", "to": [1]}]),
            P::WrongType,
            Some("/0/to"),
            None,
        ),
        (
            json!([{"key": "hero_a", "to": {"rank": [1]}}]),
            P::OutOfRange,
            Some("/0/to/rank"),
            None,
        ),
        (
            json!([{"key": "hero_b", "to": {"skill": [2]}}]),
            P::OutOfRange,
            Some("/0/to/skill"),
            None,
        ),
        (
            json!([{"key": "hero_a", "to": {"grade": [3]}}]),
            P::OutOfRange,
            Some("/0/to/grade"),
            None,
        ),
        (
            json!([{"key": "hero_a", "to": {"grade": [1, 0]}}]),
            P::OutOfRange,
            Some("/0/to/grade"),
            None,
        ),
        (
            json!([{"key": "hero_a", "to": {"grade": [1], "skill": [4]}}]),
            P::OutOfRange,
            Some("/0/to/skill"),
            None,
        ),
        (
            json!([{"key": "hero_a", "to": {"grade": 1}}]),
            P::WrongType,
            Some("/0/to/grade"),
            None,
        ),
        (
            json!([{"key": "hero_a", "to": {"grade": [1.5]}}]),
            P::WrongType,
            Some("/0/to/grade"),
            None,
        ),
        (
            json!([{"key": "hero_a", "to": {"mastery": [1]}}]),
            P::WrongType,
            Some("/0/to/mastery"),
            None,
        ),
        (
            json!([{"key": "hero_a", "to": {"mastery": {}}}]),
            P::WrongType,
            Some("/0/to/mastery"),
            None,
        ),
        (
            json!([{"key": "hero_a", "to": {"mastery": {"2": [1]}}}]),
            P::OutOfRange,
            Some("/0/to/mastery/2"),
            None,
        ),
        (
            json!([{"key": "hero_a", "to": {"mastery": {"1": [2]}}}]),
            P::OutOfRange,
            Some("/0/to/mastery/1"),
            None,
        ),
        (
            json!([{"key": "hero_b", "to": {"mastery": {"1": [1]}}}]),
            P::OutOfRange,
            Some("/0/to/mastery/1"),
            None,
        ),
    ];
    for (value, word, pointer, range) in cases {
        let value_json = set(&value);
        assert_eq!(
            refused_with(
                &page,
                Some(&tables),
                &none,
                EditRequest::Set {
                    name: RECORDS,
                    value_json: &value_json
                }
            ),
            (word, pointer.map(str::to_owned), range),
            "{value}"
        );
    }

    // A `to` whose canonical text passes 1024 bytes: every sub of both long columns.
    let mut every = serde_json::Map::new();
    for letter in ['a', 'b'] {
        let subs = (0..8)
            .map(|index| (long_sub(index), json!([1])))
            .collect::<serde_json::Map<String, Value>>();
        every.insert(long_column(letter), Value::Object(subs));
    }
    let wide = set(&json!([{"key": "k", "to": every}]));
    assert!(wide.len() > 1_024, "setup: the value is long");
    assert_eq!(
        refused_with(
            &page,
            Some(&tables),
            &none,
            EditRequest::Set {
                name: "wide",
                value_json: &wide
            }
        ),
        (P::ValueTooLarge, Some("/0/to".to_owned()), Some((0, 1_024)))
    );

    // A records entry the root does not apply answers as an unknown name.
    let empty = set(&json!([]));
    for (name, applies) in [("spare", Some(&tables)), (RECORDS, None)] {
        assert_eq!(
            refused_with(
                &page,
                applies,
                &none,
                EditRequest::Set {
                    name,
                    value_json: &empty
                }
            ),
            (P::NameUnknown, None, None),
            "{name}"
        );
    }
    assert_eq!(
        refused_with(
            &page,
            Some(&tables),
            &none,
            EditRequest::Clear { name: RECORDS }
        ),
        (P::ValueNotSet, None, None)
    );
    assert_eq!(
        refused_with(
            &page,
            Some(&tables),
            &none,
            EditRequest::Clear { name: "spare" }
        ),
        (P::NameUnknown, None, None)
    );

    // The instance's bound: 128 records of an entry the page does not declare.
    let full = (0..128)
        .map(|index| GivenRecord {
            key: format!("k{index}"),
            to: r#"{"grade":[1]}"#.to_owned(),
        })
        .collect::<Vec<_>>();
    let old = RecordsChange::Set {
        entry: "old".to_owned(),
        records: full,
    };
    let composed = compose(&none, INSTANCE_A, &old, setter()).expect("128 records are kept");
    assert_eq!(composed.rows.len(), 129, "a header and 128 records");
    let stored = StoredGoalRecords::read(Some(composed.rows.as_slice()), INSTANCE_A);
    assert!(stored.holds("old"), "{stored:?}");
    let one = set(&json!([{"key": "hero_a", "to": {"grade": [1]}}]));
    let change = checked(
        &page,
        Some(&tables),
        &stored,
        EditRequest::Set {
            name: RECORDS,
            value_json: &one,
        },
    )
    .expect("the value passes its checks");
    let change = match change {
        GoalChange::Records(change) => change,
        other @ GoalChange::Record(_) => {
            panic!("a change of the goal records record: {other:?}")
        }
    };
    let refusal =
        compose(&stored, INSTANCE_A, &change, setter()).expect_err("a 129th record is refused");
    let refusal = GoalRefusal::of_records(refusal).expect("a refusal of the value");
    assert_eq!(
        (refusal.problem, refusal.range),
        (P::RecordOutOfLimits, Some((0, 128)))
    );
    let none_of_raise = RecordsChange::Set {
        entry: RECORDS.to_owned(),
        records: Vec::new(),
    };
    assert!(compose(&stored, INSTANCE_A, &none_of_raise, setter()).is_ok());
    assert!(compose(&stored, INSTANCE_A, &old, setter()).is_ok());
    // A clear of the name the records are stored under removes them, though no entry has it.
    assert_eq!(
        checked(
            &page,
            Some(&tables),
            &stored,
            EditRequest::Clear { name: "old" }
        ),
        Ok(GoalChange::Records(RecordsChange::Clear {
            entry: "old".to_owned()
        }))
    );
}

// ---- The host part. ----

/// Places the game root below `packs`: its bundle, its `schedule.json` with the one rule (due at
/// every evaluation, producing the pool, whose valuation names its step and its importance), the
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
    state: TempDir,
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
        let host = Self::open(&state, &packs, &clock);
        Self {
            state,
            packs,
            clock,
            host,
        }
    }

    fn open(state: &TempDir, packs: &TempDir, clock: &Arc<ManualRuntimeClock>) -> RuntimeHost {
        let provider = FakeProvider::from_entries([INSTANCE_A, INSTANCE_B].map(|alias| {
            (
                alias.to_owned(),
                instance_id(),
                Arc::new(FakeState::default()),
            )
        }));
        RuntimeHost::start(
            config(state)
                .with_policy_inputs(PolicyInputSnapshot::new(facts(), policy_resources()))
                .with_runtime_clock(clock.clone())
                .with_standard_packs_root(Some(packs.path().to_owned()))
                .with_standard_pack_schedule(true),
            Arc::new(provider),
        )
        .expect("start the composer-mode host")
    }

    /// A clean stop and a new start on the same state root and game root, the Runtime clock at
    /// `at`.
    fn restart(self, at: u64) -> Self {
        let Self {
            state, packs, host, ..
        } = self;
        host.close().expect("close the composer-mode host");
        let clock = Arc::new(ManualRuntimeClock::new(at, 0));
        let host = Self::open(&state, &packs, &clock);
        Self {
            state,
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

    /// `EditGoal` of `op` on `alias` with `expected_version`, from the Lab bound with `Lab-CLI`,
    /// the Runtime clock at `at` first.
    fn edit(
        &self,
        alias: &str,
        op: GoalEdit,
        expected_version: Option<u64>,
        at: u64,
    ) -> RuntimeReceipt {
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
                expected_version,
                op,
            },
        );
        client.send(&request)
    }

    /// `alias`'s stored `session.goal_records` record as a fact reader reads it back (the
    /// instance's facts under the key); `None` without one.
    fn records(&self, alias: &str) -> Option<FactRecord> {
        let mut client = TestClient::connect(&self.host);
        let request = client.request(RuntimeOperation::InstanceFactSnapshot {
            instance: alias.to_owned(),
            key_prefix: Some(GOAL_RECORDS_FACT_KEY.to_owned()),
        });
        let receipt = client.send(&request);
        let Some(RuntimeResult::InstanceFactSnapshot { snapshot, .. }) = receipt.result() else {
            panic!("an instance fact snapshot: {receipt:?}");
        };
        snapshot
            .records
            .iter()
            .find(|record| record.key == GOAL_RECORDS_FACT_KEY)
            .cloned()
    }

    /// How many `fact.published` records of the goal records key the ledger holds.
    fn record_publications(&self) -> usize {
        self.published()
            .iter()
            .filter(|event| {
                matches!(
                    event.payload(),
                    EventPayload::Fact(FactPayload::Published(payload))
                        if payload.record().key == GOAL_RECORDS_FACT_KEY
                )
            })
            .count()
    }

    /// Every `fact.published` record the ledger holds.
    fn published(&self) -> Vec<PersistedEvent> {
        self.host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(EventType::FactPublished),
                ..EventQuery::default()
            })
            .expect("ledger events")
    }

    fn close(self) {
        assert!(
            self.host.fatal_error().expect("runtime health").is_none(),
            "the Runtime holds no fatal condition"
        );
        self.host.close().expect("close the composer-mode host");
    }
}

fn set_op(name: &str, value: &Value) -> GoalEdit {
    GoalEdit::Set {
        name: name.to_owned(),
        value_json: value.to_string(),
    }
}

fn clear_op(name: &str) -> GoalEdit {
    GoalEdit::Clear {
        name: name.to_owned(),
    }
}

/// What a taken `EditGoal` answered.
#[track_caller]
fn taken(receipt: &RuntimeReceipt) -> GoalEdited {
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(receipt.outcome().code().as_str(), EDITED, "{receipt:?}");
    let Some(RuntimeResult::GoalEdited { edited }) = receipt.result() else {
        panic!("an edited goal: {receipt:?}");
    };
    edited.clone()
}

/// A refused request: Denied with `code`, no result; its outcome.
#[track_caller]
fn denied(receipt: &RuntimeReceipt, code: &str) -> actingcommand_contract::outcome::Outcome {
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    assert_eq!(receipt.outcome().code().as_str(), code, "{receipt:?}");
    assert!(receipt.result().is_none(), "{receipt:?}");
    receipt.outcome().clone()
}

/// The `goal_problem` and the `json_pointer` of a refused `EditGoal`.
#[track_caller]
fn refused_word(receipt: &RuntimeReceipt) -> (Option<String>, Option<String>) {
    let outcome = denied(receipt, INVALID);
    let word = |key: &str| {
        outcome
            .value(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    (word("goal_problem"), word("json_pointer"))
}

fn text(value: &str) -> ContractFactScalar {
    ContractFactScalar::String(value.to_owned())
}

/// The header row of `alias`'s goal records record.
fn header_row(alias: &str) -> BTreeMap<String, ContractFactScalar> {
    BTreeMap::from([
        ("row".to_owned(), text("header")),
        (
            "schema_version".to_owned(),
            text("actingcommand.goal-records.v1"),
        ),
        ("instance".to_owned(), text(alias)),
    ])
}

/// The row of a record of `entry` for `key` to `to`, set at `at` on `lab` by `Lab-CLI`.
fn record_row(entry: &str, key: &str, to: &str, at: u64) -> BTreeMap<String, ContractFactScalar> {
    BTreeMap::from([
        ("row".to_owned(), text("record")),
        ("entry".to_owned(), text(entry)),
        ("key".to_owned(), text(key)),
        ("to".to_owned(), text(to)),
        (
            "set_unix_ms".to_owned(),
            ContractFactScalar::Integer(i64::try_from(at).expect("a fixture time")),
        ),
        ("channel".to_owned(), text("lab")),
        ("initiator".to_owned(), text(Initiator::LabCli.as_str())),
    ])
}

/// The rows a record holds.
fn rows_of(record: &FactRecord) -> Vec<BTreeMap<String, ContractFactScalar>> {
    match &record.content {
        FactContent::Inline {
            value: ContractFactValue::RecordList(rows),
        } => rows.clone(),
        other => panic!("an inline record list, not {other:?}"),
    }
}

/// `alias`'s stored rows; none without a record.
#[track_caller]
fn stored_rows(scene: &Scene, alias: &str) -> Vec<BTreeMap<String, ContractFactScalar>> {
    scene
        .records(alias)
        .map(|record| rows_of(&record))
        .unwrap_or_default()
}

/// The members of `view` as a client decodes it.
fn members(view: &GoalView) -> BTreeSet<String> {
    match serde_json::to_value(view).expect("the view as JSON") {
        Value::Object(object) => object.keys().cloned().collect(),
        other => panic!("an object, not {other}"),
    }
}

// The round trip (rulings 1 to 3, 7). On instance A, after the first cycle: no record and the
// page's version 0. A set of `raise` from the Lab is taken: a fact reader reads back the record,
// its header naming the instance and one row of the record, its `to` the canonical text, set at
// the request's time on `lab` by `Lab-CLI`; the page's version is the answer's; the page lists no
// records entry and is, entry for entry and member for member, the page it was before (today's
// client reads it as without the records). The same value again is a replay: the same version
// and event, nothing appended. Another value replaces the record's rows; an empty array leaves
// the header alone; a clear after another set does too, and a clear with nothing stored is
// `value_not_set`. A value set once more is read back unchanged after a second start of the host
// on the same state root (the seeding start) and after a third, and the page's version is then
// that record's.
#[test]
fn rr2_h1_records_are_set_read_back_replaced_cleared_and_kept_across_starts() {
    let scene = Scene::new();
    scene.cycle(T0);
    let before = scene.view(INSTANCE_A);
    assert_eq!(before.version, 0, "{before:?}");
    assert!(scene.records(INSTANCE_A).is_none());

    let first = taken(&scene.edit(INSTANCE_A, set_op(RECORDS, &raise_a()), Some(0), T0 + 1_000));
    assert!(!first.replayed);
    assert!(first.replaced_target_id.is_none());
    assert_eq!(
        stored_rows(&scene, INSTANCE_A),
        vec![
            header_row(INSTANCE_A),
            record_row(RECORDS, "hero_a", TO_A, T0 + 1_000)
        ]
    );
    let after = scene.view(INSTANCE_A);
    assert_eq!(after.version, first.version);
    assert!(
        !after.entries.iter().any(|entry| entry.name == RECORDS),
        "{after:?}"
    );
    assert_eq!(after.declaration, before.declaration);
    assert_eq!(after.entries, before.entries);
    assert_eq!(after.orphans, before.orphans);
    assert_eq!(after.remedy_parameters, before.remedy_parameters);
    assert_eq!(after.other_targets, before.other_targets);
    assert_eq!(members(&after), members(&before));

    let replay = taken(&scene.edit(INSTANCE_A, set_op(RECORDS, &raise_a()), None, T0 + 2_000));
    assert_eq!(
        (replay.version, replay.replayed, replay.event_id),
        (first.version, true, first.event_id)
    );
    assert_eq!(scene.record_publications(), 1, "nothing appended");

    let second = taken(&scene.edit(
        INSTANCE_A,
        set_op(RECORDS, &json!([{"key": "hero_b", "to": {"grade": [1]}}])),
        None,
        T0 + 3_000,
    ));
    assert!(second.version > first.version, "{second:?}");
    assert_eq!(
        stored_rows(&scene, INSTANCE_A),
        vec![
            header_row(INSTANCE_A),
            record_row(RECORDS, "hero_b", r#"{"grade":[1]}"#, T0 + 3_000)
        ]
    );

    taken(&scene.edit(INSTANCE_A, set_op(RECORDS, &json!([])), None, T0 + 4_000));
    assert_eq!(
        stored_rows(&scene, INSTANCE_A),
        vec![header_row(INSTANCE_A)]
    );
    taken(&scene.edit(INSTANCE_A, set_op(RECORDS, &raise_a()), None, T0 + 5_000));
    taken(&scene.edit(INSTANCE_A, clear_op(RECORDS), None, T0 + 6_000));
    assert_eq!(
        stored_rows(&scene, INSTANCE_A),
        vec![header_row(INSTANCE_A)]
    );
    assert_eq!(
        refused_word(&scene.edit(INSTANCE_A, clear_op(RECORDS), None, T0 + 7_000)),
        (Some(GoalProblem::ValueNotSet.as_str().to_owned()), None)
    );

    let kept = taken(&scene.edit(INSTANCE_A, set_op(RECORDS, &raise_a()), None, T0 + 8_000));
    let stored = scene.records(INSTANCE_A).expect("the record");
    assert_eq!(
        rows_of(&stored),
        vec![
            header_row(INSTANCE_A),
            record_row(RECORDS, "hero_a", TO_A, T0 + 8_000)
        ]
    );
    let scene = scene.restart(T0 + 60_000);
    assert_eq!(
        scene.records(INSTANCE_A).as_ref(),
        Some(&stored),
        "the second start"
    );
    let scene = scene.restart(T0 + 120_000);
    assert_eq!(
        scene.records(INSTANCE_A).as_ref(),
        Some(&stored),
        "the third start"
    );
    scene.cycle(T0 + 120_000);
    assert_eq!(scene.view(INSTANCE_A).version, kept.version);
    scene.close();
}

// Ruling 5: one version for the page. On instance A: a target set on the resource entry with the
// page's version 0 is taken; a set of the records entry carrying that earlier version is
// `fact_observation_version_conflict` naming the page's version and the resource target record's
// key, nothing appended; with the page's version it is taken, and the page's version is then the
// later record's. The reverse: a target set carrying the version before the records' is the
// conflict naming the goal records key; with the page's version it is taken. The page's version
// is the greater of the two records' sequences throughout (the resource target view's own version
// stays its record's), and a replay of the records answers the page's version, not the records'
// own sequence.
#[test]
fn rr2_h2_the_page_keeps_one_version_over_both_records() {
    let scene = Scene::new();
    scene.cycle(T0);
    assert_eq!(scene.view(INSTANCE_A).version, 0);
    let value = json!([{"key": "hero_a", "to": {"grade": [1]}}]);

    let target = taken(&scene.edit(
        INSTANCE_A,
        set_op(STOCK, &json!({"at_least": 50})),
        Some(0),
        T0 + 1_000,
    ));
    let first = target.version;
    assert_eq!(scene.view(INSTANCE_A).version, first);
    let stale = denied(
        &scene.edit(INSTANCE_A, set_op(RECORDS, &value), Some(0), T0 + 2_000),
        CONFLICT,
    );
    assert_eq!(stale.value("current_version"), Some(&json!(first)));
    assert_eq!(
        stale.value("fact_key"),
        Some(&json!(RESOURCE_TARGETS_FACT_KEY))
    );
    assert!(scene.records(INSTANCE_A).is_none(), "nothing appended");

    let records = taken(&scene.edit(INSTANCE_A, set_op(RECORDS, &value), Some(first), T0 + 3_000));
    let second = records.version;
    assert!(second > first, "{records:?}");
    let page = scene.view(INSTANCE_A);
    assert_eq!(page.version, second, "the greater of the two");
    assert_eq!(
        page.targets.active.as_ref().map(|active| active.version),
        Some(first),
        "the target view's own version"
    );

    let stale = denied(
        &scene.edit(
            INSTANCE_A,
            set_op(STOCK, &json!({"at_least": 60})),
            Some(first),
            T0 + 4_000,
        ),
        CONFLICT,
    );
    assert_eq!(stale.value("current_version"), Some(&json!(second)));
    assert_eq!(stale.value("fact_key"), Some(&json!(GOAL_RECORDS_FACT_KEY)));
    let target = taken(&scene.edit(
        INSTANCE_A,
        set_op(STOCK, &json!({"at_least": 60})),
        Some(second),
        T0 + 5_000,
    ));
    let third = target.version;
    assert!(third > second, "{target:?}");
    assert_eq!(scene.view(INSTANCE_A).version, third);

    let replay = taken(&scene.edit(INSTANCE_A, set_op(RECORDS, &value), Some(third), T0 + 6_000));
    assert_eq!(
        (replay.version, replay.replayed, replay.event_id),
        (third, true, records.event_id)
    );
    scene.close();
}

// Ruling 2: a fact publication of the key is refused. A `PublishFact` and a `PublishFacts` of a
// `session.goal_records` record of instance A, from the agent's envelope and from the Lab's, are
// each Denied `goal_records_formal_entry_required`; nothing is appended and the instance holds no
// such record.
#[test]
fn rr2_h3_a_fact_publication_of_the_key_is_refused() {
    let scene = Scene::new();
    scene.cycle(T0);
    let before = scene.published().len();
    let record = stored_fact(
        FactScope::Instance {
            instance_id: INSTANCE_A.to_owned(),
        },
        GOAL_RECORDS_FACT_KEY,
        ContractFactValue::RecordList(vec![header_row(INSTANCE_A)]),
        "snapshot:rr2-client",
        Vec::new(),
    );
    let publication = |batch: bool| {
        if batch {
            RuntimeOperation::PublishFacts {
                observation: actingcommand_contract::FactObservation {
                    records: vec![record.clone()],
                },
            }
        } else {
            RuntimeOperation::PublishFact {
                record: record.clone(),
            }
        }
    };
    let mut client = TestClient::connect(&scene.host);
    for batch in [false, true] {
        let agent = client.agent_request(publication(batch));
        denied(&client.send(&agent), FORMAL);
        let lab = lab_request(&client, publication(batch));
        denied(&client.send(&lab), FORMAL);
    }
    assert_eq!(scene.published().len(), before, "nothing appended");
    assert!(scene.records(INSTANCE_A).is_none());
    scene.close();
}

// Ruling 6: when the root changes under stored records. Two records of `raise` are stored on
// instance A. The root's `goal-tables.json` is rewritten without `hero_b`, and the next check
// reads it (a set naming `hero_b` is now `not_an_option`): the stored record is read back as it
// was, byte for byte; the clear of `raise` removes both records. A record set again, then the
// root's `goals.json` rewritten without the records entry (a set of `raise` is now
// `name_unknown`): the record stays stored, and the clear of the name removes it, declared or
// not.
#[test]
fn rr2_h4_records_the_root_no_longer_declares_stay_stored_and_the_clear_removes_them() {
    let scene = Scene::new();
    scene.cycle(T0);
    let both = json!([{"key": "hero_a", "to": {"grade": [2]}},
                      {"key": "hero_b", "to": {"grade": [1]}}]);
    taken(&scene.edit(INSTANCE_A, set_op(RECORDS, &both), None, T0 + 1_000));
    let stored = scene.records(INSTANCE_A).expect("the record");
    assert_eq!(
        rows_of(&stored),
        vec![
            header_row(INSTANCE_A),
            record_row(RECORDS, "hero_a", r#"{"grade":[2]}"#, T0 + 1_000),
            record_row(RECORDS, "hero_b", r#"{"grade":[1]}"#, T0 + 1_000)
        ]
    );

    write_json(
        &tables_path(scene.packs.path()),
        &tables_file(vec![heroes(false)]),
    );
    let checked_at = T0 + CHECK_INTERVAL_MS;
    scene.cycle(checked_at);
    assert_eq!(
        refused_word(&scene.edit(
            INSTANCE_A,
            set_op(RECORDS, &json!([{"key": "hero_b", "to": {"grade": [1]}}])),
            None,
            checked_at + 1_000,
        )),
        (
            Some(GoalProblem::NotAnOption.as_str().to_owned()),
            Some("/0/key".to_owned())
        ),
        "the root in force no longer has hero_b"
    );
    assert_eq!(
        scene.records(INSTANCE_A).as_ref(),
        Some(&stored),
        "read back as it is"
    );
    taken(&scene.edit(INSTANCE_A, clear_op(RECORDS), None, checked_at + 2_000));
    assert_eq!(
        stored_rows(&scene, INSTANCE_A),
        vec![header_row(INSTANCE_A)]
    );

    let one = json!([{"key": "hero_a", "to": {"grade": [1]}}]);
    taken(&scene.edit(INSTANCE_A, set_op(RECORDS, &one), None, checked_at + 3_000));
    let kept = scene.records(INSTANCE_A).expect("the record");
    write_json(&goals_path(scene.packs.path()), &goals(false));
    let undeclared_at = checked_at + CHECK_INTERVAL_MS;
    scene.cycle(undeclared_at);
    assert_eq!(
        refused_word(&scene.edit(
            INSTANCE_A,
            set_op(RECORDS, &one),
            None,
            undeclared_at + 1_000
        )),
        (Some(GoalProblem::NameUnknown.as_str().to_owned()), None),
        "the root in force no longer declares the entry"
    );
    assert_eq!(scene.records(INSTANCE_A).as_ref(), Some(&kept));
    taken(&scene.edit(INSTANCE_A, clear_op(RECORDS), None, undeclared_at + 2_000));
    assert_eq!(
        stored_rows(&scene, INSTANCE_A),
        vec![header_row(INSTANCE_A)]
    );
    scene.close();
}

// Two instances keep their own records. Instance A's `raise` holds `hero_a`, instance B's
// `hero_b`: each instance's record names its own instance and holds its own row, and each page's
// version is its own record's. A clear on A leaves A the header alone and B's record and page as
// they were.
#[test]
fn rr2_h5_two_instances_keep_their_own_records() {
    let scene = Scene::new();
    scene.cycle(T0);
    let on_a = taken(&scene.edit(
        INSTANCE_A,
        set_op(RECORDS, &json!([{"key": "hero_a", "to": {"grade": [2]}}])),
        None,
        T0 + 1_000,
    ));
    let on_b = taken(&scene.edit(
        INSTANCE_B,
        set_op(RECORDS, &json!([{"key": "hero_b", "to": {"grade": [1]}}])),
        None,
        T0 + 2_000,
    ));
    assert_eq!(
        stored_rows(&scene, INSTANCE_A),
        vec![
            header_row(INSTANCE_A),
            record_row(RECORDS, "hero_a", r#"{"grade":[2]}"#, T0 + 1_000)
        ]
    );
    let b_record = scene.records(INSTANCE_B).expect("B's record");
    assert_eq!(
        rows_of(&b_record),
        vec![
            header_row(INSTANCE_B),
            record_row(RECORDS, "hero_b", r#"{"grade":[1]}"#, T0 + 2_000)
        ]
    );
    assert_eq!(scene.view(INSTANCE_A).version, on_a.version);
    assert_eq!(scene.view(INSTANCE_B).version, on_b.version);

    taken(&scene.edit(INSTANCE_A, clear_op(RECORDS), None, T0 + 3_000));
    assert_eq!(
        stored_rows(&scene, INSTANCE_A),
        vec![header_row(INSTANCE_A)]
    );
    assert_eq!(scene.records(INSTANCE_B).as_ref(), Some(&b_record));
    assert_eq!(scene.view(INSTANCE_B).version, on_b.version);
    scene.close();
}
