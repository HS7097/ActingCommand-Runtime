// SPDX-License-Identifier: AGPL-3.0-only

// #455 RR3c (0.12.98; register §1.543; the coordinator's rulings 1 to 7 of 11-28 in its work order
// `rd-1123/brief-455-rr3c.md`; contracts/scheduling/README.md, "Pool lines" and "Needs computed
// from the records"; contracts/standard-pack-goal-tables.md, "What is computed"), the host's half.
// A run that was told enough turns to close a goal's gap leaves the pool it produced for that goal
// unread until a reading observed at or after its end:
// - the needs computed from the records: a holding left unread adds nothing below its item, the
//   whole need of the item on its pool, with its own note (p1);
// - through the real admission and settlement path: a run handed a `gap_remaining` its production
//   closes ends; the next evaluation, the pool not read again, holds the task whose trigger reads
//   the pool (its key suggested with `fact_stale`); a restart of the host on the same state root,
//   whose journal is rebuilt from the ledger, gives the same; a reading after the run's end
//   releases the pool (h1).
//
// The tables, keys, items and pools are made up; times are fixed Unix milliseconds on a manual
// Runtime clock, and nothing sleeps.

use super::*;
use crate::PolicyTrigger;
use crate::goal_needs::{NeedsSources, RecordRoots, RecordsRoot, instance_needs};
use crate::goal_records::{GOAL_RECORDS_SCHEMA_VERSION, StoredGoalRecords};
use actingcommand_contract::{
    FactScalar, GOAL_RECORDS_FACT_KEY, RuntimeChannel, StandardPackGoalTables,
    parse_standard_pack_goal_tables,
};
use actingcommand_policy::{
    NeedHolding, PolicyCode, PolicyEvaluation, PoolNeed, PoolNeedKind, ProgressListUnknown,
    RecordNeedsNote, TaskDecision,
};
use serde_json::{Value, json};

const ALIAS: &str = POLICY_INSTANCE_ALIAS;
/// When the fixture's lists are first read: ten seconds before the fixture's time.
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
const SHA: &str = "fixture-tables-c";
/// The host fixture's pool, read from rows of [`LIST_KEY`], on which the need lands.
const LISTED: &str = "pool.listed";
const LIST_KEY: &str = "inventory.list.example_kind_a";
const TASK: &str = "fixture.observe";
/// The instance's game, by which the fixture's roots are found.
const GAME: &str = "fixture-game-a";
/// How long the run takes on the Runtime clock: shorter than the dispatch lease's lifetime in the
/// tests' configuration, as LF-1's host gate takes a dispatch from its admission to its end.
const RUN_MS: u64 = 3_000;
/// The Runtime clock when the run is settled, its end.
const SETTLED_AT: u64 = POLICY_NOW_UNIX_MS + RUN_MS;

// ---- The needs computed from the records

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

fn own_scope() -> FactScope {
    FactScope::Instance {
        instance_id: ALIAS.to_owned(),
    }
}

fn text(value: &str) -> FactScalar {
    FactScalar::String(value.to_owned())
}

/// The list `fact_key` of `rows`, read at [`READ_AT`], fresh for [`FRESH_MS`], at confidence 900.
fn list(fact_key: &str, rows: Vec<BTreeMap<String, FactScalar>>) -> FactRecord {
    let mut fact = stored_fact(
        own_scope(),
        fact_key,
        ContractFactValue::RecordList(rows),
        &format!("snapshot:rr3c:{fact_key}"),
        Vec::new(),
    );
    fact.observed_at_unix_ms = READ_AT;
    fact.expires_at_unix_ms = Some(READ_AT + FRESH_MS);
    fact.confidence_milli = 900;
    fact
}

/// The roster with `unit_a` at `grade`.
fn roster(grade: i64) -> FactRecord {
    list(
        ROSTER,
        vec![BTreeMap::from([
            ("unit".to_owned(), text("unit_a")),
            ("grade".to_owned(), FactScalar::Integer(grade)),
        ])],
    )
}

/// The header row of the instance's `session.goal_records` record.
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

/// A record row of `unit_a` to the canonical `to`, set at the fixture's time on the CLI.
fn record_row(to: &str) -> BTreeMap<String, FactScalar> {
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
    ])
}

fn tables(document: &Value) -> StandardPackGoalTables {
    parse_standard_pack_goal_tables(document.to_string().as_bytes(), TABLES_GAME)
        .unwrap_or_else(|issues| panic!("the fixture's tables read: {issues:?}"))
}

/// The root whose records entry [`ENTRY`] names the table [`TABLE`] of `parsed`.
fn root(parsed: StandardPackGoalTables) -> RecordsRoot {
    RecordsRoot {
        entries: BTreeMap::from([(ENTRY.to_owned(), TABLE.to_owned())]),
        tables: Some((Arc::new(parsed), SHA.to_owned())),
    }
}

/// The tables of p1: `top` is made 1 for 4 `low` and 300 `coin`; a key's one grade step needs 10
/// `top`.
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

// p1, the needs' reader (ruling 3). A record that raises `unit_a` one grade needs 10 of `top`, an
// item made 1 for 4 `low` and 300 `coin`. Its holding known, 4: 4 of the need are covered and the 6
// left are made, 24 `low` and 1800 `coin` (RR3's own numbers). Its holding unread, a run told
// enough turns to close a goal's gap on `top`'s pool having produced into it since its reading: the
// whole 10 lands on `top`'s pool (`Unsplit`), nothing below it is taken apart (no `low`, no
// `coin`), and the one note is the unread holding's, which an evaluation says as
// `goal_records_holding_unknown` with `fact_stale`.
#[test]
fn rr3c_p1_an_unread_holding_adds_nothing_below_its_item() {
    let rows = vec![header(), record_row(r#"{"grade":[1]}"#)];
    let stored = StoredGoalRecords::read(Some(rows.as_slice()), ALIAS);
    assert!(
        matches!(&stored, StoredGoalRecords::Read(taken) if taken.len() == 1),
        "the fixture's records read: {stored:?}"
    );
    let needs_with = |holding: NeedHolding| {
        let sources = Sources {
            lists: BTreeMap::from([(ROSTER.to_owned(), roster(0))]),
            holding,
        };
        instance_needs(
            &stored,
            POLICY_NOW_UNIX_MS,
            Some(&root(tables(&recipe_tables()))),
            &sources,
        )
        .expect("an instance with records has needs")
    };

    let known = needs_with(NeedHolding::Known {
        held: 4,
        fresh_until_unix_ms: None,
    });
    assert_eq!(
        known.pools(),
        &pools(&[
            ("pool.coin", 1_800, PoolNeedKind::Lands),
            ("pool.low", 24, PoolNeedKind::Lands),
            ("pool.top", 4, PoolNeedKind::Covered),
        ])
    );
    assert!(known.notes().is_empty(), "{:?}", known.notes());

    let unread = needs_with(NeedHolding::Unread);
    assert_eq!(
        unread.pools(),
        &pools(&[("pool.top", 10, PoolNeedKind::Unsplit)])
    );
    assert_eq!(
        unread.notes(),
        [RecordNeedsNote::HoldingUnread {
            item: "top".to_owned(),
            pool_id: "pool.top".to_owned(),
        }]
    );
    assert_ne!(unread.identity(), known.identity());
}

// ---- The host

/// #455 P2's fixture catalog declared V2: its pool [`LISTED`] read from rows `item_a` × 200 and
/// `item_b` × 400 of [`LIST_KEY`], valued (step 1, base 1000, gap weight 100); its task due while
/// the pool holds at least 1000 and producing 2000 of it a run, consuming nothing; its profiles
/// without goals.
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
            document["tasks"][0]["consumes"] = json!([]);
            document["tasks"][0]["produces"] = json!([{
                "pool_id": LISTED,
                "direction": "produce",
                "amount": 2_000,
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

/// What the host's needs read of the instance's game: `coin`, held in [`LISTED`]; a class's grade
/// ladder `[0] [1] [2]` of 1000 `coin` and then 2000, read from the roster.
fn coin_roots() -> RecordRoots {
    let parsed = tables(&json!({
        "schema_version": "actingcommand.goal-tables.v1",
        "game": TABLES_GAME,
        "items": {"coin": {"pool": LISTED}},
        "tables": [{
            "name": TABLE,
            "progress": [{"fact_key": ROSTER, "key_column": "unit", "held_by": true, "columns": ["grade"]}],
            "columns": [{"name": "grade", "read": ["grade"], "per": "class"}],
            "classes": {"all": {"grade": {
                "positions": [[0], [1], [2]],
                "steps": [{"coin": 1_000}, {"coin": 2_000}]
            }}},
            "keys": {"unit_a": {"class": "all"}}
        }]
    }));
    RecordRoots::from([(GAME.to_owned(), root(parsed))])
}

/// A start of the policy instance's host over `state` on `clock`, its needs reading
/// [`coin_roots`].
fn open(state: &TempDir, instance: InstanceId, clock: &Arc<ManualRuntimeClock>) -> RuntimeHost {
    let host = RuntimeHost::start(
        config(state).with_runtime_clock(Arc::<ManualRuntimeClock>::clone(clock)),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("the runtime host");
    host.set_goal_record_roots_for_test(Some(coin_roots()))
        .expect("the needs' roots");
    host
}

/// One evaluated cycle at `unix_ms`, the Runtime clock moved there and the host's capacity
/// sampled, over the fixture's facts, no pool snapshot configured.
fn evaluate_at(host: &RuntimeHost, clock: &ManualRuntimeClock, unix_ms: u64) -> PolicyEvaluation {
    clock.set_unix_ms(unix_ms);
    host.capacity_sampler_for_test().expect("capacity owner")().expect("capacity sample");
    let mut resources = policy_resources();
    resources.pools.clear();
    host.evaluate_policy_cycle_with_test_inputs(
        &policy_facts(),
        &resources,
        EvaluationTime {
            unix_ms,
            monotonic_ms: unix_ms,
        },
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
        .find(|entry| entry.task_id == TASK && entry.instance_id.as_deref() == Some(ALIAS))
        .unwrap_or_else(|| panic!("{TASK}'s decision: {evaluation:#?}"))
}

fn intent(evaluation: &PolicyEvaluation) -> Option<&DispatchIntent> {
    evaluation
        .dispatch_intents
        .iter()
        .find(|dispatch| dispatch.task_id == TASK && dispatch.instance_id == ALIAS)
}

/// Whether `evaluation`'s decision of the task suggests reading the list's key for `fact_stale`.
fn stale(evaluation: &PolicyEvaluation) -> bool {
    decision(evaluation)
        .detection_suggestions
        .iter()
        .any(|suggestion| suggestion.fact_key == LIST_KEY && suggestion.reason == "fact_stale")
}

/// The list of [`LIST_KEY`], `item_a` 3 and `item_b` 2 (so the pool reads 1400), read at
/// `observed_at` under the snapshot `snapshot`, fresh for [`FRESH_MS`].
fn list_read_at(observed_at: u64, snapshot: &str) -> FactRecord {
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
    fact.source_snapshot_id = snapshot.to_owned();
    fact.observed_at_unix_ms = observed_at;
    fact.expires_at_unix_ms = Some(observed_at + FRESH_MS);
    fact
}

/// The instance's `session.goal_records` record of one record of `unit_a` to grade 2, as the goal
/// edit stores it (no expiry), written at [`READ_AT`].
fn records() -> FactRecord {
    let to = r#"{"grade":[2]}"#;
    let mut fact = stored_fact(
        own_scope(),
        GOAL_RECORDS_FACT_KEY,
        ContractFactValue::RecordList(vec![header(), record_row(to)]),
        "snapshot:rr3c-records",
        Vec::new(),
    );
    fact.observed_at_unix_ms = READ_AT;
    fact.expires_at_unix_ms = None;
    fact.ttl_policy = None;
    fact.confidence_milli = 1_000;
    fact.resource_bundle_hash = format!("{:x}", Sha256::digest(to.as_bytes()));
    fact
}

// h1, the span through the host and a start (rulings 1, 2 and 5; the work order's test 7). With the
// records raising `unit_a` from grade 0 to 2, the needs are 3000 `coin` on the pool, which holds
// 1400: the task, producing 2000 a run, is dispatched handed `gap_remaining` 1600 at its production
// entry 0 and no `run_cap` (one turn: 2000·1000 ≥ 1600·1000). Admitted, run and settled, the run
// ends; the next evaluation, the list still fresh and not read again, holds the task: its trigger
// reads the pool unknown and suggests the list's key with `fact_stale`, and nothing is dispatched.
// A restart of the host on the same state root rebuilds the list from the ledger and gives the same
// at two evaluations; a list read after the run's end releases the pool.
#[test]
fn rr3c_h1_a_run_told_enough_to_close_its_gap_leaves_its_pool_unread_across_a_start() {
    let state = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = open(&state, instance, &clock);
    host.activate_policy_catalog(&listed_sources())
        .expect("activate the fixture catalog");
    evaluate_at(&host, &clock, POLICY_NOW_UNIX_MS);
    host.publish_list_reading_for_test(list_read_at(READ_AT, "snapshot:rr3c-list"))
        .expect("the pool's list");
    host.publish_list_reading_for_test(roster(0))
        .expect("the roster");
    host.publish_goal_records_for_test(records())
        .expect("the records");

    let evaluation = evaluate_at(&host, &clock, POLICY_NOW_UNIX_MS);
    let intent_now = intent(&evaluation)
        .unwrap_or_else(|| panic!("the task is due: {evaluation:#?}"))
        .clone();
    let reasons = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent_now.reason_chain_id)
        .expect("the intent's reason chain")
        .clone();
    let handed = &reasons
        .reasons
        .iter()
        .find(|given| given.outcome.is(PolicyCode::PolicyDispatchParameters))
        .unwrap_or_else(|| panic!("the dispatch's parameters: {reasons:#?}"))
        .outcome;
    assert_eq!(
        handed.value("gaps_remaining"),
        Some(&json!([{"effect_index": 0, "gap_remaining": 1_600}])),
        "{handed:?}"
    );
    assert_eq!(handed.value("run_cap"), None, "{handed:?}");
    record_policy_approval(&host, &intent_now);
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_policy_dispatch(&intent_now, &reasons, &policy_context(&host, &intent_now))
        .expect("admit the task")
    else {
        panic!("the task is granted");
    };
    clock.advance(RUN_MS);
    host.complete_policy_dispatch(&intent_now.decision_id)
        .expect("complete the dispatch");
    host.release_policy_dispatch_lease_for_test(&context)
        .expect("end the dispatch's lease");

    let held = evaluate_at(&host, &clock, SETTLED_AT + 5_000);
    assert!(intent(&held).is_none(), "{held:#?}");
    assert!(
        stale(&held),
        "the list read before the run is stale: {held:#?}"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");

    let clock = Arc::new(ManualRuntimeClock::new(SETTLED_AT + 6_000, 0));
    let host = open(&state, instance, &clock);
    for unix_ms in [SETTLED_AT + 7_000, SETTLED_AT + 8_000] {
        let restarted = evaluate_at(&host, &clock, unix_ms);
        assert!(intent(&restarted).is_none(), "{restarted:#?}");
        assert!(stale(&restarted), "the same after a start: {restarted:#?}");
    }

    clock.set_unix_ms(SETTLED_AT + 10_000);
    host.publish_list_reading_for_test(list_read_at(SETTLED_AT + 10_000, "snapshot:rr3c-again"))
        .expect("the list read again");
    let released = evaluate_at(&host, &clock, SETTLED_AT + 20_000);
    assert!(
        !stale(&released),
        "a reading after the run's end releases the pool: {released:#?}"
    );
    assert!(
        decision(&released)
            .reasons
            .iter()
            .all(|given| given.outcome.code().as_str() != "trigger_unknown"),
        "the trigger reads the pool again: {released:#?}"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}
