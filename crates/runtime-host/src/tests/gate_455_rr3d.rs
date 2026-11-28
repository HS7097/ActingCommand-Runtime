// SPDX-License-Identifier: AGPL-3.0-only

// #455 RR3d (0.12.101; register §1.548; the coordinator's rulings 1 to 6 of 11-28 in its work order
// `rd-1123/brief-455-rr3d.md`; contracts/scheduling/README.md, "Pool lines", "At each evaluation";
// contracts/resource-targets.md, "A pool a run was told enough to close the gap on"), the host's
// half. Before each evaluation the host notes every admitted dispatch its journal holds at the
// inputs' ledger position, in the order of their admission (`PolicyHost::handed_runs`):
// - the list (h1 to h3, from synthetic journals, compared with the list noted from the dispatches
//   the ruling keeps, in their order): a run that failed at its entry (its record a condition whose
//   failure identity carries the entry mark) is passed over and its pair's earlier dispatch stands;
//   every other end is noted: a success, a failure after the main package started, a failure at
//   the entry its settlement did not mark, one a start's reconciliation settled, an interrupted
//   settlement, a stop, a run still going, and a failure at the entry recorded after the inputs'
//   position (h1); a dispatch that failed at its entry between two that count adds nothing (h2);
//   the list holds what the journal keeps of a pair, its latest 64 admitted dispatches (h3);
// - through the real admission and settlement path: a run handed a `run_cap` that failed at its
//   entry leaves the pool it consumes read at the next evaluation, the same run failing after its
//   main package started leaves it unread (h4); a run handed a `gap_remaining` it closes that
//   failed at its entry leaves the pool it produces read and the goal applied, failing after its
//   main package started unread (h5); two runs each handed a `gap_remaining` of 1600 producing 1000
//   leave the pool read after the first and unread after the second, the same at two evaluations
//   after a start that rebuilds the journal from the ledger; a list read after their end releases
//   it (h6).
//
// The pool is read from rows of a list (#455 P2's fixture: `item_a` × 200 and `item_b` × 400 of
// `inventory.list.example_kind_a`, read as 1400); the task is catalog-a's, due while the pool holds
// 1000. Names, keys, tables and items are made up; times are fixed Unix milliseconds on a manual
// Runtime clock, and nothing sleeps.

use super::*;
use crate::PolicyTrigger;
use crate::failure_identity::{FailureIdentity, IdentityFrame, IdentityPackage};
use crate::goal_needs::{RecordRoots, RecordsRoot};
use crate::goal_records::GOAL_RECORDS_SCHEMA_VERSION;
use crate::policy_host::{SyntheticHandedDispatch, synthetic_handed_runs_for_test};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Outcome, Record};
use actingcommand_contract::{
    FactScalar, GOAL_RECORDS_FACT_KEY, PerformanceContext, PolicyConditionSettlement,
    PolicyExecutionEventData, PolicyFailureRecord, RuntimeChannel, StandardPackGoalTables,
    parse_standard_pack_goal_tables,
};
use actingcommand_policy::{
    HandedRuns, PolicyCode, PolicyEvaluation, REMEDY_JOURNAL_MAX_RUNS, TaskDecision,
};
use serde_json::{Value, json};

// ---- The list, from synthetic journals

/// The synthetic pair's task and instance.
const SYNTHETIC_TASK: &str = "task-synthetic-a";
const SYNTHETIC_INSTANCE: &str = "instance-synthetic-a";
/// When the earlier and the later synthetic run ended.
const EARLIER_END: u64 = 2_000;
const LATER_END: u64 = 3_000;
/// A failure identity's frame digest.
const DIGEST: &str = "0123456789ab";
const PAGE_UNKNOWN: &str = "contained_task_page_unknown";

/// A `policy_dispatch_parameters` reason handed `gap_remaining` `gap` at the production entry 0
/// and `run_cap` (none: no `run_cap`).
fn parameters(gap: i64, run_cap: Option<i64>) -> Outcome {
    let outcome = Outcome::new(PolicyCode::PolicyDispatchParameters)
        .gaps_remaining([Record::new().effect_index(0_usize).gap_remaining(gap)]);
    match run_cap {
        Some(run_cap) => outcome.run_cap(run_cap),
        None => outcome,
    }
}

/// A failure identity whose base is the page-unknown code with the frame mark `frame`.
fn identity(frame: IdentityFrame) -> String {
    FailureIdentity {
        base: PAGE_UNKNOWN.to_owned(),
        key: "0".repeat(12),
        main: IdentityPackage {
            id_hash: "0a1b2c3d".to_owned(),
            hash: "x".to_owned(),
        },
        layers: Vec::new(),
        frame,
    }
    .encode()
}

fn succeeded() -> PolicyExecutionOutcome {
    PolicyExecutionOutcome::Succeeded { runtime_ms: 1_000 }
}

/// A recoverable failure of `outcome` recorded with the identity `failure_identity`, settled as a
/// condition (its counts given back, as a failure at the entry is) when `condition`.
fn failed(
    outcome: Outcome,
    failure_identity: Option<String>,
    condition: bool,
) -> PolicyExecutionOutcome {
    PolicyExecutionOutcome::Failed {
        failure: PolicyFailureRecord {
            outcome,
            failure_identity,
            reported_success: false,
            original_class: PolicyFailureClass::Recoverable,
            effective_class: PolicyFailureClass::Recoverable,
            consecutive_same_error: u16::from(!condition),
            escalation_streak: 0,
            performance_tax_exempt: false,
            retry_attempt: 0,
            disposition: PolicyFailureDisposition::Continue,
            retry_at_unix_ms: None,
            runtime_ms: 1_000,
            sensitive: false,
            perf_context: Box::new(PerformanceContext::legacy_unavailable()),
            condition: condition.then_some(PolicyConditionSettlement {
                refunded: true,
                count: 1,
                expires_at_unix_ms: LATER_END + 86_400_000,
            }),
        },
    }
}

/// A failure the settlement settled as one at its run's entry: a condition, its identity's frame
/// the entry mark.
fn failed_at_entry() -> PolicyExecutionOutcome {
    failed(
        Outcome::new(SharedCode::ContainedTaskPageUnknown),
        Some(identity(IdentityFrame::Entry(DIGEST.to_owned()))),
        true,
    )
}

/// A dispatch of the synthetic pair whose recorded reasons are `told` and whose run, when `end`
/// is given, ended at `ended_at` so.
fn dispatch(
    told: &Outcome,
    end: Option<PolicyExecutionOutcome>,
    ended_at: u64,
) -> SyntheticHandedDispatch {
    SyntheticHandedDispatch {
        task_id: SYNTHETIC_TASK,
        instance_id: SYNTHETIC_INSTANCE,
        reasons: vec![told.clone()],
        ended: end.map(|outcome| (ended_at, outcome)),
    }
}

// h1, ruling 1 at the list (rulings 1 and 3; the work order's test 1 and the study's R1 host
// tests). A pair's earlier dispatch, handed `run_cap` 3 and `gap_remaining` 2, succeeded; its
// later one, handed the same, ended as each case says. A failure at the entry is passed over: the
// list is the one noted from the earlier dispatch alone, which stands as the pair's latest. Every
// other end is noted after it, as before this slice. A failure at the entry whose record comes
// after the inputs' ledger position (its completion at the sequence 8, the position 7) is a run
// that has not ended there: noted, running.
#[test]
fn rr3d_h1_a_run_that_failed_at_its_entry_is_passed_over_and_every_other_end_is_noted() {
    let told = parameters(2, Some(3));
    let unique = || Some(identity(IdentityFrame::Unique(DIGEST.to_owned())));
    let other = || Outcome::new(RuntimeErrorCode::RuntimeUnavailable);
    let page_unknown = || Outcome::new(SharedCode::ContainedTaskPageUnknown);
    let cases = [
        ("a success", Some(succeeded()), false),
        ("a failure at the entry", Some(failed_at_entry()), true),
        (
            "a failure after the main package started",
            Some(failed(page_unknown(), unique(), false)),
            false,
        ),
        (
            "a failure at the entry its settlement did not mark",
            Some(failed(other(), unique(), false)),
            false,
        ),
        (
            "a failure a start's reconciliation settled",
            Some(failed(page_unknown(), None, false)),
            false,
        ),
        (
            "an interrupted settlement",
            Some(failed(
                Outcome::new(SharedCode::PolicySettlementInterrupted),
                None,
                false,
            )),
            false,
        ),
        (
            "a stop",
            Some(PolicyExecutionOutcome::Stopped {
                outcome: other(),
                runtime_ms: 1_000,
            }),
            false,
        ),
        ("a run that has not ended", None, false),
    ];
    for (case, end, passed_over) in cases {
        let later_end = end.as_ref().map(|_| LATER_END);
        let given = synthetic_handed_runs_for_test(
            vec![
                dispatch(&told, Some(succeeded()), EARLIER_END),
                dispatch(&told, end, LATER_END),
            ],
            u64::MAX,
        );
        let mut expected = HandedRuns::default();
        expected.note_dispatch(
            SYNTHETIC_TASK,
            SYNTHETIC_INSTANCE,
            Some(EARLIER_END),
            [&told],
        );
        if !passed_over {
            expected.note_dispatch(SYNTHETIC_TASK, SYNTHETIC_INSTANCE, later_end, [&told]);
        }
        assert_eq!(given, expected, "{case}");
    }

    let given = synthetic_handed_runs_for_test(
        vec![
            dispatch(&told, Some(succeeded()), EARLIER_END),
            dispatch(&told, Some(failed_at_entry()), LATER_END),
        ],
        7,
    );
    let mut expected = HandedRuns::default();
    expected.note_dispatch(
        SYNTHETIC_TASK,
        SYNTHETIC_INSTANCE,
        Some(EARLIER_END),
        [&told],
    );
    expected.note_dispatch(SYNTHETIC_TASK, SYNTHETIC_INSTANCE, None, [&told]);
    assert_eq!(given, expected, "recorded after the inputs' position");
}

// h2, a dispatch that failed at its entry between two that count (the work order's test 2). Three
// dispatches of the pair, each handed 3 turns of a gap of 7 (one unit a run would make the three 9
// ≥ 7), the second failed at its entry: the list is the one noted from the first and the third, 6
// < 7 (the policy gate `rr3d_t1` judges such a list).
#[test]
fn rr3d_h2_a_dispatch_that_failed_at_its_entry_between_two_that_count_adds_nothing() {
    let told = parameters(7, Some(3));
    let given = synthetic_handed_runs_for_test(
        vec![
            dispatch(&told, Some(succeeded()), EARLIER_END),
            dispatch(&told, Some(failed_at_entry()), LATER_END),
            dispatch(&told, Some(succeeded()), LATER_END + 1_000),
        ],
        u64::MAX,
    );
    let mut expected = HandedRuns::default();
    expected.note_dispatch(
        SYNTHETIC_TASK,
        SYNTHETIC_INSTANCE,
        Some(EARLIER_END),
        [&told],
    );
    expected.note_dispatch(
        SYNTHETIC_TASK,
        SYNTHETIC_INSTANCE,
        Some(LATER_END + 1_000),
        [&told],
    );
    assert_eq!(given, expected);
}

// h3, the bound (ruling 4, a limit written down). 65 dispatches of the pair, each handed one turn
// of a gap of 65 and each completed in turn: the journal keeps the pair's latest 64 admitted
// dispatches, so the list is the one noted from those 64; the earliest is not counted (64 < 65).
#[test]
fn rr3d_h3_the_list_holds_what_the_journal_keeps_of_a_pair() {
    let kept = u64::try_from(REMEDY_JOURNAL_MAX_RUNS).expect("the journal's bound");
    let told = parameters(i64::try_from(kept + 1).expect("the gap"), None);
    let given = synthetic_handed_runs_for_test(
        (0..=kept)
            .map(|index| dispatch(&told, Some(succeeded()), EARLIER_END + index))
            .collect(),
        u64::MAX,
    );
    let mut expected = HandedRuns::default();
    for index in 1..=kept {
        expected.note_dispatch(
            SYNTHETIC_TASK,
            SYNTHETIC_INSTANCE,
            Some(EARLIER_END + index),
            [&told],
        );
    }
    assert_eq!(given, expected);
}

// ---- Through the host

const ALIAS: &str = POLICY_INSTANCE_ALIAS;
const TASK: &str = "fixture.observe";
/// The pool, read from rows of [`LIST_KEY`].
const LISTED: &str = "pool.listed";
const LIST_KEY: &str = "inventory.list.example_kind_a";
/// When the fixture's lists are first read: ten seconds before the fixture's time.
const READ_AT: u64 = POLICY_NOW_UNIX_MS - 10_000;
/// How long a list read stays fresh: an hour, past h6's second dispatch.
const FRESH_MS: u64 = 3_600_000;
/// How long a run takes on the Runtime clock: shorter than the dispatch lease's lifetime in the
/// tests' configuration (5 s), as LF-1's and RR3c's host gates take a dispatch to its end.
const RUN_MS: u64 = 3_000;
/// The Runtime clock when the first run is settled, its end.
const SETTLED_AT: u64 = POLICY_NOW_UNIX_MS + RUN_MS;
/// h6: when the second dispatch is evaluated and admitted, ten minutes after the first run's end,
/// past the longest interval catalog-a's activity profile samples after an admission.
const SECOND_AT: u64 = SETTLED_AT + 600_000;
/// The instance's game, by which the fixture's roots are found.
const GAME: &str = "fixture-game-a";
/// The table's `held_by` list, which gives `grade`.
const ROSTER: &str = "inventory.list.fixture_roster";
/// The records entry, and the table it names.
const ENTRY: &str = "raise";
const TABLE: &str = "units";
/// The game the fixture's tables files are written for.
const TABLES_GAME: &str = "neutral";
const SHA: &str = "fixture-tables-d";
/// The goal the needs give the pool when it is read: 3000 of it, 1400 held.
const APPLIED: &str = "goal_records@pool.listed:applied current=1400 at_least=3000 gap=1600 ";
/// The same goal while the pool is unread.
const PENDING: &str = "goal_records@pool.listed:pending reason=expired gap_weight=pending";

/// [`LISTED`], the policy instance's, read from rows `item_a` × 200 and `item_b` × 400 of
/// [`LIST_KEY`], valued (step 1, base 1000, gap weight 100).
fn list_pool() -> Value {
    json!({
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
    })
}

/// catalog-a declared V2 (its events valid from 0), its pools [`list_pool`] alone, its profiles
/// without goals, its task due while [`LISTED`] holds at least 1000, then shaped by `shape`.
fn sources(shape: impl Fn(&mut Value)) -> CatalogSources {
    let mut catalog = policy_sources(1);
    for source in [
        &mut catalog.tasks,
        &mut catalog.pools,
        &mut catalog.activity,
        &mut catalog.timeline,
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
            let task = &mut document["tasks"][0];
            task["trigger"] = json!({
                "kind": "resource_projection",
                "pool_id": LISTED,
                "comparison": "greater_than_or_equal",
                "value": 1_000
            });
            shape(task);
        }
        if document.get("pools").is_some() {
            document["pools"] = json!([list_pool()]);
        }
        if let Some(profiles) = document.get_mut("profiles").and_then(Value::as_array_mut) {
            for profile in profiles {
                profile["goals"] = json!([]);
            }
        }
        source.bytes = serde_json::to_vec_pretty(&document).expect("the fixture's bytes");
    }
    catalog
}

/// [`sources`] whose task consumes 400 of the pool a run and reserves 200, producing nothing:
/// read at 1400, SS-6 hands it ⌊(1400 − 200) ÷ 400⌋ = 3 turns (LF-1's host fixture).
fn consuming() -> CatalogSources {
    sources(|task| {
        task["consumes"] = json!([{
            "pool_id": LISTED,
            "direction": "consume",
            "amount": 400,
            "observation_source": "scan_verified",
            "confidence_milli": 1000
        }]);
        task["produces"] = json!([]);
        task["reserves"] = json!([{"pool_id": LISTED, "at_least": 200}]);
    })
}

/// [`sources`] whose task produces `amount` of the pool a run, consuming nothing (RR3c's host
/// fixture produces 2000).
fn producing(amount: u64) -> CatalogSources {
    sources(move |task| {
        task["consumes"] = json!([]);
        task["produces"] = json!([{
            "pool_id": LISTED,
            "direction": "produce",
            "amount": amount,
            "observation_source": "scan_verified",
            "confidence_milli": 1000
        }]);
    })
}

fn own_scope() -> FactScope {
    FactScope::Instance {
        instance_id: ALIAS.to_owned(),
    }
}

fn text(value: &str) -> FactScalar {
    FactScalar::String(value.to_owned())
}

/// The list `fact_key` of `rows`, read at `observed_at` under the snapshot `snapshot`, fresh for
/// [`FRESH_MS`], at confidence 900.
fn list(
    fact_key: &str,
    rows: Vec<BTreeMap<String, FactScalar>>,
    observed_at: u64,
    snapshot: &str,
) -> FactRecord {
    let mut fact = stored_fact(
        own_scope(),
        fact_key,
        ContractFactValue::RecordList(rows),
        snapshot,
        Vec::new(),
    );
    fact.observed_at_unix_ms = observed_at;
    fact.expires_at_unix_ms = Some(observed_at + FRESH_MS);
    // The shared fixture's lifetime policy admits two minutes at most, and an observation whose
    // lifetime lies outside its policy is refused (`fact_observation_invalid`): this list's own
    // policy admits its hour.
    if let Some(policy) = fact.ttl_policy.as_mut() {
        policy.maximum_ms = FRESH_MS;
    }
    fact.confidence_milli = 900;
    fact
}

/// The list of [`LIST_KEY`], `item_a` 3 and `item_b` 2 (so the pool reads 1400), read at
/// `observed_at` under the snapshot `snapshot`.
fn list_read_at(observed_at: u64, snapshot: &str) -> FactRecord {
    let row = |key: &str, count: i64| {
        BTreeMap::from([
            ("id".to_owned(), text(key)),
            ("count".to_owned(), FactScalar::Integer(count)),
        ])
    };
    list(
        LIST_KEY,
        vec![row("item_a", 3), row("item_b", 2)],
        observed_at,
        snapshot,
    )
}

/// The roster with `unit_a` at grade 0, read at [`READ_AT`].
fn roster() -> FactRecord {
    list(
        ROSTER,
        vec![BTreeMap::from([
            ("unit".to_owned(), text("unit_a")),
            ("grade".to_owned(), FactScalar::Integer(0)),
        ])],
        READ_AT,
        "snapshot:rr3d:roster",
    )
}

/// The instance's `session.goal_records` record of one record of `unit_a` to grade 2, as the goal
/// edit stores it (no expiry), written at [`READ_AT`].
fn records() -> FactRecord {
    let to = r#"{"grade":[2]}"#;
    let header = BTreeMap::from([
        ("row".to_owned(), text("header")),
        (
            "schema_version".to_owned(),
            text(GOAL_RECORDS_SCHEMA_VERSION),
        ),
        ("instance".to_owned(), text(ALIAS)),
    ]);
    let record = BTreeMap::from([
        ("row".to_owned(), text("record")),
        ("entry".to_owned(), text(ENTRY)),
        ("key".to_owned(), text("unit_a")),
        ("to".to_owned(), text(to)),
        (
            "set_unix_ms".to_owned(),
            FactScalar::Integer(i64::try_from(POLICY_NOW_UNIX_MS).expect("the fixture's time")),
        ),
        ("channel".to_owned(), text(RuntimeChannel::Cli.as_str())),
    ]);
    let mut fact = stored_fact(
        own_scope(),
        GOAL_RECORDS_FACT_KEY,
        ContractFactValue::RecordList(vec![header, record]),
        "snapshot:rr3d-records",
        Vec::new(),
    );
    fact.observed_at_unix_ms = READ_AT;
    fact.expires_at_unix_ms = None;
    fact.ttl_policy = None;
    fact.confidence_milli = 1_000;
    fact.resource_bundle_hash = format!("{:x}", Sha256::digest(to.as_bytes()));
    fact
}

/// What the host's needs read of the instance's game: `coin`, held in [`LISTED`]; a class's grade
/// ladder `[0] [1] [2]` of 1000 `coin` and then 2000, read from the roster: grade 0 to 2 needs
/// 3000.
fn coin_roots() -> RecordRoots {
    let document = json!({
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
    });
    let parsed: StandardPackGoalTables =
        parse_standard_pack_goal_tables(document.to_string().as_bytes(), TABLES_GAME)
            .unwrap_or_else(|issues| panic!("the fixture's tables read: {issues:?}"));
    RecordRoots::from([(
        GAME.to_owned(),
        RecordsRoot {
            entries: BTreeMap::from([(ENTRY.to_owned(), TABLE.to_owned())]),
            tables: Some((Arc::new(parsed), SHA.to_owned())),
        },
    )])
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

/// A fresh host on `state` with `catalog` active and the pool's list read at [`READ_AT`]; with
/// `needs`, the roster and the records too.
fn started(
    state: &TempDir,
    clock: &Arc<ManualRuntimeClock>,
    catalog: &CatalogSources,
    needs: bool,
) -> RuntimeHost {
    let host = open(state, instance_id(), clock);
    host.activate_policy_catalog(catalog)
        .expect("activate the fixture catalog");
    evaluate_at(&host, clock, POLICY_NOW_UNIX_MS);
    host.publish_list_reading_for_test(list_read_at(READ_AT, "snapshot:rr3d-list"))
        .expect("the pool's list");
    if needs {
        host.publish_list_reading_for_test(roster())
            .expect("the roster");
        host.publish_goal_records_for_test(records())
            .expect("the records");
    }
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

/// How a test settles a run: a success; a failure the live settlement settles as one at the
/// run's entry; the same failure after the run's main package started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Settle {
    Succeeded,
    AtEntry,
    PastEntry,
}

/// The task's dispatch of `evaluation` (the catalog approved first when `approve`), admitted, its
/// run [`RUN_MS`] long settled as `settle` says, and its lease ended: the dispatch's recorded
/// `policy_dispatch_parameters` and its execution record. The failure is the input the live
/// settlement builds for a run that failed in its prerequisite layer on a screen the layer does
/// not know (its `F` unique), armed as a failure at the entry or as one after the main package
/// started.
fn run(
    host: &RuntimeHost,
    clock: &ManualRuntimeClock,
    evaluation: &PolicyEvaluation,
    approve: bool,
    settle: Settle,
) -> (Outcome, PolicyExecutionEventData) {
    let intent_now = intent(evaluation)
        .unwrap_or_else(|| panic!("the task is due: {evaluation:#?}"))
        .clone();
    let reasons = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent_now.reason_chain_id)
        .expect("the intent's reason chain")
        .clone();
    let handed = reasons
        .reasons
        .iter()
        .find(|given| given.outcome.is(PolicyCode::PolicyDispatchParameters))
        .unwrap_or_else(|| panic!("the dispatch's parameters: {reasons:#?}"))
        .outcome
        .clone();
    if approve {
        record_policy_approval(host, &intent_now);
    }
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_policy_dispatch(&intent_now, &reasons, &policy_context(host, &intent_now))
        .expect("admit the task")
    else {
        panic!("the task is granted");
    };
    clock.advance(RUN_MS);
    let failure = PolicyExecutionInput::Failed {
        outcome: Outcome::new(SharedCode::ContainedTaskPageUnknown),
        failure_identity: Some(identity(IdentityFrame::unique(&intent_now.decision_id))),
        class: PolicyFailureClass::Recoverable,
    };
    let input = match settle {
        Settle::Succeeded => PolicyExecutionInput::Succeeded,
        Settle::AtEntry => {
            host.settle_next_policy_run_at_entry_for_test()
                .expect("arm the settlement");
            failure
        }
        Settle::PastEntry => {
            host.settle_next_policy_run_past_its_entry_for_test()
                .expect("arm the settlement");
            failure
        }
    };
    let settled = host
        .record_policy_dispatch_outcome(&intent_now.decision_id, &input)
        .expect("the settlement");
    host.release_policy_dispatch_lease_for_test(&context)
        .expect("end the dispatch's lease");
    (handed, settled)
}

/// Whether the execution record `settled` is a failure its settlement settled as one at its run's
/// entry (a condition, the entry mark).
fn settled_at_entry(settled: &PolicyExecutionEventData) -> bool {
    matches!(
        &settled.outcome,
        PolicyExecutionOutcome::Failed { failure }
            if crate::policy_control::entry_blocked(failure)
    )
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

/// Whether `evaluation`'s decision of the task suggests reading the list's key for `fact_stale`:
/// its trigger read the pool unknown.
fn stale(evaluation: &PolicyEvaluation) -> bool {
    decision(evaluation)
        .detection_suggestions
        .iter()
        .any(|suggestion| suggestion.fact_key == LIST_KEY && suggestion.reason == "fact_stale")
}

/// Whether `evaluation`'s decision of the task names `code`.
fn has(evaluation: &PolicyEvaluation, code: PolicyCode) -> bool {
    decision(evaluation)
        .reasons
        .iter()
        .any(|given| given.outcome.is(code))
}

/// Whether `evaluation`'s decision of the task read the consumed pool unknown: its trigger, or,
/// rerun at once past its trigger (R22, a failure whose `F` is unique), its reserve gate, which
/// then holds it with no amount after.
fn consumed_unknown(evaluation: &PolicyEvaluation) -> bool {
    stale(evaluation)
        || decision(evaluation).reasons.iter().any(|given| {
            given.outcome.is(PolicyCode::ResourceReserveInsufficient)
                && given.outcome.value("amount_after").is_none()
        })
}

/// The detail of the `resource_targets` reason of `evaluation`'s decision of the task, the goal's
/// item, when the decision carries one.
fn goal(evaluation: &PolicyEvaluation) -> Option<&str> {
    decision(evaluation)
        .reasons
        .iter()
        .find(|given| given.outcome.is(PolicyCode::ResourceTargets))
        .map(|given| given.detail.as_str())
}

/// Whether `evaluation`'s decision of the task read the produced pool unread: its trigger, or,
/// rerun at once past its trigger, the goal on the pool, then pending.
fn produced_unknown(evaluation: &PolicyEvaluation) -> bool {
    stale(evaluation) || goal(evaluation) == Some(PENDING)
}

// h4, ruling 1 on the consumed side (the work order's test 1). The task, read at 1400, is
// dispatched handed `run_cap` 3. Its run fails at its entry: its record is a condition whose
// identity carries the entry mark, and the next evaluation reads the pool (no `fact_stale`, no
// `trigger_unknown`, no reserve hold for want of a reading); it dispatches nothing, the instance
// being blocked by that failure. The same run failing after its main package started leaves the
// pool unread, as before this slice: the next evaluation reads it unknown and dispatches nothing.
#[test]
fn rr3d_h4_a_capped_run_that_failed_at_its_entry_leaves_its_consumed_pool_read() {
    for settle in [Settle::AtEntry, Settle::PastEntry] {
        let state = TempDir::new().expect("tempdir");
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let host = started(&state, &clock, &consuming(), false);
        let evaluation = evaluate_at(&host, &clock, POLICY_NOW_UNIX_MS);
        let (handed, settled) = run(&host, &clock, &evaluation, true, settle);
        assert_eq!(
            handed.value("run_cap"),
            Some(&json!(3)),
            "{settle:?}: {handed:?}"
        );
        assert_eq!(
            settled_at_entry(&settled),
            settle == Settle::AtEntry,
            "{settle:?}: {settled:?}"
        );

        let after = evaluate_at(&host, &clock, SETTLED_AT + 5_000);
        assert!(intent(&after).is_none(), "{settle:?}: {after:#?}");
        assert_eq!(
            consumed_unknown(&after),
            settle == Settle::PastEntry,
            "{settle:?}: {after:#?}"
        );
        if settle == Settle::AtEntry {
            assert!(!has(&after, PolicyCode::TriggerUnknown), "{after:#?}");
        }
        assert!(host.fatal_error().expect("runtime health").is_none());
        host.close().expect("close the host");
    }
}

// h5, ruling 1 on the produced side (the study's R1 host tests). With the records raising `unit_a`
// from grade 0 to 2, the needs are 3000 of the pool, which holds 1400: the task, producing 2000 a
// run, is dispatched handed `gap_remaining` 1600 and no `run_cap` (one turn closes it). Its run
// fails at its entry: the next evaluation reads the pool (no `fact_stale`) and the goal on it is
// applied, its gap 1600 as before the run; nothing is dispatched to the blocked instance. The same
// run failing after its main package started leaves the pool unread, as before this slice.
#[test]
fn rr3d_h5_a_run_that_failed_at_its_entry_leaves_the_pool_it_was_told_to_close_read() {
    for settle in [Settle::AtEntry, Settle::PastEntry] {
        let state = TempDir::new().expect("tempdir");
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let host = started(&state, &clock, &producing(2_000), true);
        let evaluation = evaluate_at(&host, &clock, POLICY_NOW_UNIX_MS);
        let (handed, settled) = run(&host, &clock, &evaluation, true, settle);
        assert_eq!(
            handed.value("gaps_remaining"),
            Some(&json!([{"effect_index": 0, "gap_remaining": 1_600}])),
            "{settle:?}: {handed:?}"
        );
        assert_eq!(handed.value("run_cap"), None, "{settle:?}: {handed:?}");
        assert_eq!(
            settled_at_entry(&settled),
            settle == Settle::AtEntry,
            "{settle:?}: {settled:?}"
        );

        let after = evaluate_at(&host, &clock, SETTLED_AT + 5_000);
        if settle == Settle::AtEntry {
            assert!(!stale(&after), "{after:#?}");
            assert!(
                goal(&after).is_some_and(|item| item.starts_with(APPLIED)),
                "{after:#?}"
            );
            assert!(intent(&after).is_none(), "{after:#?}");
        } else {
            assert!(produced_unknown(&after), "{after:#?}");
        }
        assert!(host.fatal_error().expect("runtime health").is_none());
        host.close().expect("close the host");
    }
}

// h6, ruling 2 through the host and a start (the study's R2 host tests). The task produces 1000 a
// run. Its first dispatch is handed `gap_remaining` 1600 and no `run_cap`: one turn, 1000 < 1600,
// and after its run the pool stays read (no `fact_stale`). Ten minutes later, the list not read
// again, it is dispatched again, told the same; after that run the pool is unread (2000 ≥ 1600):
// the next evaluation holds the task (its trigger suggests the list's key with `fact_stale`, no
// intent). A start of the host on the same state root rebuilds the journal from the ledger and
// gives the same at two evaluations; a list read after the second run's end releases the pool.
#[test]
fn rr3d_h6_two_runs_short_of_the_gap_leave_the_pool_unread_across_a_start() {
    let state = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = open(&state, instance, &clock);
    host.activate_policy_catalog(&producing(1_000))
        .expect("activate the fixture catalog");
    evaluate_at(&host, &clock, POLICY_NOW_UNIX_MS);
    host.publish_list_reading_for_test(list_read_at(READ_AT, "snapshot:rr3d-list"))
        .expect("the pool's list");
    host.publish_list_reading_for_test(roster())
        .expect("the roster");
    host.publish_goal_records_for_test(records())
        .expect("the records");
    let told = json!([{"effect_index": 0, "gap_remaining": 1_600}]);

    let first = evaluate_at(&host, &clock, POLICY_NOW_UNIX_MS);
    let (handed, _) = run(&host, &clock, &first, true, Settle::Succeeded);
    assert_eq!(handed.value("gaps_remaining"), Some(&told), "{handed:?}");
    assert_eq!(handed.value("run_cap"), None, "{handed:?}");
    let between = evaluate_at(&host, &clock, SETTLED_AT + 5_000);
    assert!(!stale(&between), "one run short of the gap: {between:#?}");

    let second = evaluate_at(&host, &clock, SECOND_AT);
    let (handed, _) = run(&host, &clock, &second, false, Settle::Succeeded);
    assert_eq!(handed.value("gaps_remaining"), Some(&told), "{handed:?}");
    assert_eq!(handed.value("run_cap"), None, "{handed:?}");
    let ended = SECOND_AT + RUN_MS;
    let held = evaluate_at(&host, &clock, ended + 5_000);
    assert!(intent(&held).is_none(), "{held:#?}");
    assert!(stale(&held), "two runs reach the gap: {held:#?}");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");

    let clock = Arc::new(ManualRuntimeClock::new(ended + 6_000, 0));
    let host = open(&state, instance, &clock);
    for unix_ms in [ended + 7_000, ended + 8_000] {
        let restarted = evaluate_at(&host, &clock, unix_ms);
        assert!(intent(&restarted).is_none(), "{restarted:#?}");
        assert!(stale(&restarted), "the same after a start: {restarted:#?}");
    }

    clock.set_unix_ms(ended + 10_000);
    host.publish_list_reading_for_test(list_read_at(ended + 10_000, "snapshot:rr3d-again"))
        .expect("the list read again");
    let released = evaluate_at(&host, &clock, ended + 20_000);
    assert!(
        !stale(&released),
        "a reading after the runs' end releases the pool: {released:#?}"
    );
    assert!(
        !has(&released, PolicyCode::TriggerUnknown),
        "the trigger reads the pool again: {released:#?}"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}
