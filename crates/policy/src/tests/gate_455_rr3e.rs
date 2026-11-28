// SPDX-License-Identifier: AGPL-3.0-only

// #455 RR3e (0.12.104; register §1.560; the coordinator's rulings of 11-29 in its work order
// `rd-1123/brief-455-rr3e.md`, after the read-only study `rd-1123/study-rr3c-gaps-1128.md`;
// contracts/scheduling/README.md, "Fact Refresh" and "Pool lines"; contracts/resource-targets.md,
// "A pool a run was told enough to close the gap on"), ruling 1: a judged producer the net-benefit
// gate holds as not positive while a goal that covers it waits on a pool the production told under
// the goal's gap left unread asks the fact refresh for that pool's reading, with the pool's key and
// `fact_stale`; from there it is the existing fact refresh. A producer that passes on its own value
// asks nothing (test 2), and a pool read from rows of a list is not asked of the fact refresh, whose
// reader never reads a list (test 3).
//
// The fixture is RR3c's (`gate_455_rr3c`): the instance `instance-a` of one game; the capped pool
// `pool.neutral` (valued 1000 milli a unit at the step 1, no regeneration, its capacity read from
// its fact, 1000, its stop line 40 below and its hard line 900 below it); `pool.tiered`, read from
// its own fact, and `pool.listed`, read from rows of a list, both without a valuation, writing
// `target_defaults` (`S` 10, `I` 1000) and `goal_tiers` (`I_n` 100 000, `C` 40). The records need 2
// of the producer's pool, held 0, so the first round sends a producer of `V` 900 with `B` 1900 and
// tells it 2 at `run_cap` 2, which leaves the pool unread; the capped pool is read again after the
// run's end. A recognition maintenance package of `instance-a` marks `pool.tiered`'s key, and the
// fact refresh is on. Names are made up; no clock is read.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, instance};
use crate::{
    CandidateEligibility, CappedMemory, CatalogDocumentSource, CatalogSources, ChainHandoverInputs,
    CompiledCatalog, DecisionReason, DispatchIntent, EvaluationFacts, EvaluationResources,
    EvaluationTime, FACT_REFRESH_TASK_ID, FactRefreshInputs, FactRefreshReader,
    FactRefreshSettings, FactValue, GivenPoolReading, GivenPoolReadings, GivenReadingState,
    HandedRuns, HostResourceSnapshot, InstanceHolds, InstancePoolLines, InstanceRecordNeeds,
    LeftOutFacts, NoProgressGuards, ObservedFact, OwedReruns, PolicyCode, PolicyEvaluation,
    PolicyEvaluationError, PoolNeed, PoolNeedKind, RecordNeeds, RemedyPlan, RunSlots,
    SCHEDULING_SCHEMA_VERSION_V2, ScopeSelector, TaskDecision, compile_catalog,
    evaluate_with_left_out_facts, project_fact_pools_with_readings,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const OPERATION: &str = "operation.run";
const MINUTE_MS: u64 = 60_000;
/// How long a run lasts: it ends half a minute after the round that dispatched it.
const RUN_MS: u64 = 30_000;
/// The capped pool, read from `READING_KEY`, its capacity from `CAPACITY_KEY`.
const POOL: &str = "pool.neutral";
const READING_KEY: &str = "resource.neutral";
const CAPACITY_KEY: &str = "resource.neutral.capacity";
/// The capped pool's capacity.
const CAPACITY: i64 = 1000;
/// The pool of `instance-a` read from its own fact, `TIERED_KEY`.
const TIERED: &str = "pool.tiered";
const TIERED_KEY: &str = "resource.tiered";
/// The pool of `instance-a` read from rows of the list `LIST_KEY`.
const LISTED: &str = "pool.listed";
const LIST_KEY: &str = "inventory.list.example_kind_a";
/// `S` and `I`: the defaults of every goal on the two pools without a valuation.
const DEFAULTS: (u64, u64) = (10, 1000);
/// `I_n` and `C`: their `goal_tiers`.
const TIERS: (u64, u64) = (100_000, 40);
/// The sha256 the fixture's needs name as their tables'.
const SHA: &str = "fixture-tables-e";
/// `instance-a`'s recognition maintenance package.
const READER: &str = "package.reader";
/// The declared duration of every task, `E` without a snapshot.
const E: u64 = 60_000;

// Test 1 (the study's first R3a test). `task_t` of `V` 900 consumes 1 of the capped pool at a cost
// of 1000 and produces 1 of `pool.tiered`, on which the records need 2: sent once, `B` 1900. Its run
// ends and the capped pool is read again after that end; `pool.tiered`'s reading is the one before
// the run. The next round holds it as not positive (`B` −100) while the goal on `pool.tiered` is
// pending, and it asks for the pool: the recognition maintenance package is requested for its key
// with `fact_stale` (`fact_refresh_requested`) and dispatched, and `task_t` waits for it
// (`fact_refresh_pending`). Before this slice nothing asked: no trigger reads the pool. Once a
// reading observed after the run's end shows the need met, the producer is held as not positive
// again, and asks nothing, as the goal no longer waits on the pool.
#[test]
fn rr3e_t1_a_producer_held_not_positive_asks_for_the_pool_its_pending_goal_is_on() {
    let mut world = World::new(catalog(vec![producer(900, TIERED)])).with_needs(TIERED, 2);
    world.read(READING_KEY, 500, NOW - MINUTE_MS);
    world.read(TIERED_KEY, 0, NOW - MINUTE_MS);
    let sent = world.evaluate();
    assert_eq!(dispatched(&sent), [("task_t", A)], "{sent:#?}");
    world.run(&sent, "task_t");
    let ended = NOW + RUN_MS;
    world.read(READING_KEY, 499, ended + 1_000);

    let asked = world.evaluate();
    assert_eq!(
        dispatched(&asked),
        [(FACT_REFRESH_TASK_ID, A)],
        "{asked:#?}"
    );
    let waits = decision(&asked, "task_t");
    assert!(
        has(waits, PolicyCode::PolicyBenefitNonpositive),
        "{waits:#?}"
    );
    assert_eq!(
        reason(waits, PolicyCode::ResourceTargets).detail,
        "goal_records@pool.tiered:pending reason=expired gap_weight=pending"
    );
    let pending = reason(waits, PolicyCode::FactRefreshPending);
    for (key, value) in [
        ("fact_key", json!(TIERED_KEY)),
        ("unknown_reason", json!("fact_stale")),
        ("package_id", json!(READER)),
    ] {
        assert_eq!(
            pending.outcome.value(key),
            Some(&value),
            "{key}: {pending:?}"
        );
    }
    let requested = reason(
        decision(&asked, FACT_REFRESH_TASK_ID),
        PolicyCode::FactRefreshRequested,
    );
    assert_eq!(
        requested.outcome.value("fact_key"),
        Some(&json!(TIERED_KEY))
    );
    assert_eq!(
        requested.outcome.value("unknown_reason"),
        Some(&json!("fact_stale"))
    );

    world.read(TIERED_KEY, 2, ended + 2_000);
    world.now += MINUTE_MS;
    let met = world.evaluate();
    assert!(dispatched(&met).is_empty(), "{met:#?}");
    let held = decision(&met, "task_t");
    assert!(has(held, PolicyCode::PolicyBenefitNonpositive), "{held:#?}");
    assert!(refresh_codes(held).is_empty(), "{held:#?}");
    assert!(no_reader_decision(&met), "{met:#?}");
}

// Test 2 (the study's second R3a test). `task_t` of `V` 10 000, the same rounds: on the second its
// goal on `pool.tiered` is pending and it passes on its own value (`B` 9000), so it is dispatched
// and asks nothing: no `fact_refresh_*` reason and no decision of the recognition maintenance
// package.
#[test]
fn rr3e_t2_a_producer_that_passes_on_its_own_value_asks_for_nothing() {
    let mut world = World::new(catalog(vec![producer(10_000, TIERED)])).with_needs(TIERED, 2);
    world.read(READING_KEY, 500, NOW - MINUTE_MS);
    world.read(TIERED_KEY, 0, NOW - MINUTE_MS);
    let first = world.evaluate();
    assert_eq!(dispatched(&first), [("task_t", A)], "{first:#?}");
    world.run(&first, "task_t");
    world.read(READING_KEY, 499, NOW + RUN_MS + 1_000);

    let second = world.evaluate();
    assert_eq!(dispatched(&second), [("task_t", A)], "{second:#?}");
    let passes = decision(&second, "task_t");
    assert_eq!(
        reason(passes, PolicyCode::ResourceTargets).detail,
        "goal_records@pool.tiered:pending reason=expired gap_weight=pending"
    );
    assert!(refresh_codes(passes).is_empty(), "{passes:#?}");
    assert!(no_reader_decision(&second), "{second:#?}");
}

// Test 3 (the study's third R3a test). Test 1's rounds with the producer's pool `pool.listed`, read
// from rows of a list (held 0, the list read a minute before NOW): on the second round `task_t` is
// held as not positive while the goal on the pool is pending, and the fact refresh is not asked:
// no `fact_refresh_*` reason, no decision of the recognition maintenance package, nothing
// dispatched. The list's key is left out for its instance with the reading, as before this slice.
#[test]
fn rr3e_t3_a_pool_read_from_rows_of_a_list_is_not_asked_of_the_fact_refresh() {
    let mut world = World::new(catalog(vec![producer(900, LISTED)])).with_needs(LISTED, 2);
    world.read(READING_KEY, 500, NOW - MINUTE_MS);
    world.read_list(0, NOW - MINUTE_MS);
    let sent = world.evaluate();
    assert_eq!(dispatched(&sent), [("task_t", A)], "{sent:#?}");
    world.run(&sent, "task_t");
    world.read(READING_KEY, 499, NOW + RUN_MS + 1_000);

    let after = world.evaluate();
    assert!(dispatched(&after).is_empty(), "{after:#?}");
    let waits = decision(&after, "task_t");
    assert!(
        has(waits, PolicyCode::PolicyBenefitNonpositive),
        "{waits:#?}"
    );
    assert_eq!(
        reason(waits, PolicyCode::ResourceTargets).detail,
        "goal_records@pool.listed:pending reason=expired gap_weight=pending"
    );
    assert!(refresh_codes(waits).is_empty(), "{waits:#?}");
    assert!(no_reader_decision(&after), "{after:#?}");
}

// ---- The world a test drives ----

/// The catalog, the Runtime's memory of the capped episodes, the readings of `instance-a`, the
/// reading given to the pool read from rows of a list, the needs the records compute (by pool and
/// need), the runs the host notes and the evaluation's time.
struct World {
    catalog: CompiledCatalog,
    memory: CappedMemory,
    readings: Vec<ObservedFact>,
    listed: GivenPoolReadings,
    needs: Vec<(&'static str, u64)>,
    handed: HandedRuns,
    now: u64,
}

impl World {
    /// The world of `instance-a` at NOW: nothing read but the capped pool's capacity, no records,
    /// no run.
    fn new(compiled: CompiledCatalog) -> Self {
        Self {
            catalog: compiled,
            memory: CappedMemory::default(),
            readings: Vec::new(),
            listed: GivenPoolReadings::default(),
            needs: Vec::new(),
            handed: HandedRuns::default(),
            now: NOW,
        }
    }

    /// The same world with records whose computed needs land `need` on `pool`.
    fn with_needs(mut self, pool: &'static str, need: u64) -> Self {
        self.needs.push((pool, need));
        self
    }

    /// `key` of `instance-a` reads `value`, observed at `at`, in place of its reading.
    fn read(&mut self, key: &str, value: i64, at: u64) {
        self.readings.retain(|held| held.fact_key != key);
        self.readings.push(reading(key, value, at));
    }

    /// The list `pool.listed` is read from was read at `at` and its rows sum to `value`, as the
    /// host gives the reading, valid a day.
    fn read_list(&mut self, value: u64, at: u64) {
        let mut given = GivenPoolReadings::default();
        given.give(
            LISTED,
            GivenPoolReading {
                instance_id: A.to_owned(),
                fact_key: LIST_KEY.to_owned(),
                state: GivenReadingState::Read {
                    value,
                    observed_at_unix_ms: at,
                    expires_at_unix_ms: Some(at + DAY_MS),
                },
            },
        );
        self.listed = given;
    }

    fn facts(&self) -> EvaluationFacts {
        let mut observed = vec![reading(CAPACITY_KEY, CAPACITY, NOW - HOUR_MS)];
        observed.extend(self.readings.iter().cloned());
        EvaluationFacts {
            ledger_position: 7,
            fact_snapshot_id: "snapshot:fixture-455-rr3e".to_owned(),
            facts: observed,
            outcomes: Vec::new(),
            tasks: Vec::new(),
            instances: vec![instance(A, &[])],
            priority_offsets: Vec::new(),
        }
    }

    /// The needs of `instance-a`'s records, of the tables [`SHA`], set an hour before NOW, of no
    /// expiry and with no notes.
    fn record_needs(&self) -> RecordNeeds {
        let landing = self
            .needs
            .iter()
            .map(|(pool, need)| {
                (
                    (*pool).to_owned(),
                    PoolNeed {
                        need: *need,
                        kind: PoolNeedKind::Lands,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut computed = RecordNeeds::default();
        computed.give(
            A,
            InstanceRecordNeeds::new(
                Some(SHA.to_owned()),
                NOW - HOUR_MS,
                landing,
                Vec::new(),
                None,
            ),
        );
        computed
    }

    /// One round now by the Runtime's entry, with its memory, the list's reading, the needs and the
    /// noted runs beside the left-out facts, the fact refresh on with `instance-a`'s reader, every
    /// candidate admitted.
    fn evaluate(&mut self) -> PolicyEvaluation {
        let facts = self.facts();
        let resources = project_fact_pools_with_readings(
            &self.catalog,
            &facts,
            &EvaluationResources {
                pools: Vec::new(),
                hosts: vec![host()],
            },
            &self.listed,
        );
        let mut left_out = LeftOutFacts::default();
        left_out.give_pool_readings(self.listed.clone());
        left_out.give_record_needs(self.record_needs());
        left_out.give_handed_runs(self.handed.clone());
        evaluate_with_left_out_facts::<PolicyEvaluationError>(
            &self.catalog,
            &facts,
            &resources,
            time(self.now),
            11,
            &BTreeSet::new(),
            &refresh_inputs(),
            &RemedyPlan::default(),
            &ChainHandoverInputs::default(),
            &RunSlots::default(),
            &OwedReruns::default(),
            &InstanceHolds::default(),
            &NoProgressGuards::default(),
            &mut self.memory,
            &InstancePoolLines::default(),
            &left_out,
            |_, _, _| Ok(false),
            |_: &DispatchIntent| Ok(CandidateEligibility::Eligible),
        )
        .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
    }

    /// The round's dispatch of `task_id` on `instance-a` ran: it ends half a minute after the
    /// round, the host notes it from its recorded reasons, and the next round is a minute after
    /// this one.
    fn run(&mut self, evaluation: &PolicyEvaluation, task_id: &str) {
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.task_id == task_id && intent.instance_id == A)
            .unwrap_or_else(|| panic!("a dispatch of {task_id}: {evaluation:#?}"));
        let chain = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .unwrap_or_else(|| panic!("the dispatch's reason chain: {evaluation:#?}"));
        self.handed.note_dispatch(
            &intent.task_id,
            &intent.instance_id,
            Some(self.now + RUN_MS),
            chain.reasons.iter().map(|given| &given.outcome),
        );
        self.now += MINUTE_MS;
    }
}

/// The integer `value` of `key` in the scope of `instance-a`, observed at `at`, of no expiry, at
/// full confidence.
fn reading(key: &str, value: i64, at: u64) -> ObservedFact {
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: A.to_owned(),
        },
        fact_key: key.to_owned(),
        value: FactValue::Integer(value),
        observed_at_unix_ms: at,
        expires_at_unix_ms: None,
        confidence_milli: 1_000,
    }
}

fn time(at: u64) -> EvaluationTime {
    EvaluationTime {
        unix_ms: at,
        monotonic_ms: at,
    }
}

fn host() -> HostResourceSnapshot {
    HostResourceSnapshot {
        host_id: instance(A, &[]).host_id,
        cpu_available_milli: 1_000,
        gpu_available_milli: 1_000,
        io_available_milli: 1_000,
        host_responsiveness_basis_points: 10_000,
        third_party_pressure_basis_points: 0,
        heavy_dispatch_limit: 1,
        active_heavy_dispatches: 0,
        heavy_threshold_milli: crate::DEFAULT_HEAVY_THRESHOLD_MILLI,
    }
}

/// The fact refresh on, `instance-a`'s recognition maintenance package bound, marking
/// `pool.tiered`'s key (a mark never names a list's key).
fn refresh_inputs() -> FactRefreshInputs {
    FactRefreshInputs {
        settings: FactRefreshSettings {
            enabled: true,
            min_interval_ms: 300_000,
            daily_limit: 96,
            failure_backoff_ms: 600_000,
            max_consecutive_failures: 3,
            expected_duration_ms: crate::FACT_REFRESH_EXPECTED_DURATION_MS,
        },
        readers: vec![FactRefreshReader {
            instance_id: A.to_owned(),
            package_id: READER.to_owned(),
            marked_keys: BTreeSet::from([TIERED_KEY.to_owned()]),
            dispatched_today: 0,
            last_failure_code: None,
            problem: None,
        }],
        instance_facts_loaded_at_unix_ms: Some(NOW - DAY_MS),
        instance_holds: BTreeMap::new(),
        idle_closed: BTreeMap::new(),
    }
}

// ---- Assertions ----

/// The round's dispatches, by task and instance.
fn dispatched(evaluation: &PolicyEvaluation) -> Vec<(&str, &str)> {
    evaluation
        .dispatch_intents
        .iter()
        .map(|intent| (intent.task_id.as_str(), intent.instance_id.as_str()))
        .collect()
}

fn decision<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|entry| entry.task_id == task_id && entry.instance_id.as_deref() == Some(A))
        .unwrap_or_else(|| panic!("a decision for {task_id} on {A}: {evaluation:#?}"))
}

/// Whether no decision of the round is the recognition maintenance package's.
fn no_reader_decision(evaluation: &PolicyEvaluation) -> bool {
    evaluation
        .decisions
        .iter()
        .all(|entry| entry.task_id != FACT_REFRESH_TASK_ID)
}

fn has(entry: &TaskDecision, code: PolicyCode) -> bool {
    entry.reasons.iter().any(|given| given.outcome.is(code))
}

fn reason(entry: &TaskDecision, code: PolicyCode) -> &DecisionReason {
    entry
        .reasons
        .iter()
        .find(|given| given.outcome.is(code))
        .unwrap_or_else(|| panic!("a {} reason on {entry:#?}", code.as_str()))
}

/// The codes of the decision's reasons that are the fact refresh's (`fact_refresh_*`).
fn refresh_codes(entry: &TaskDecision) -> Vec<&str> {
    entry
        .reasons
        .iter()
        .map(|given| given.outcome.code().as_str())
        .filter(|code| code.starts_with("fact_refresh_"))
        .collect()
}

// ---- The catalog ----

fn effect(pool: &str, direction: &str, amount: u64) -> Value {
    json!({
        "pool_id": pool,
        "direction": direction,
        "amount": amount,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    })
}

/// Due every hour (one minute past the hour at NOW).
fn hourly() -> Value {
    json!({
        "kind": "clock",
        "schedule": {
            "kind": "interval",
            "clock_source": {"kind": "local"},
            "every_ms": HOUR_MS,
            "anchor_ms": 0
        }
    })
}

/// `task_t` of the game, judged by net benefit, of the fixed value `value`, hourly, consuming 1 of
/// the capped pool and producing 1 of `pool` a run.
fn producer(value: i64, pool: &str) -> Value {
    json!({
        "id": "task_t",
        "scope": {"kind": "game", "game_id": GAME},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": "procedure.task_t",
        "priority": 100,
        "trigger": hourly(),
        "feedback_stop": {
            "kind": "outcome",
            "task_id": "task_t",
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": [effect(POOL, "consume", 1)],
        "produces": [effect(pool, "produce", 1)],
        "on_failure": {
            "action": "continue",
            "retry_limit": 2,
            "retry_backoff_ms": 30000,
            "escalation_threshold": 3
        },
        "sensitive": false,
        "next_run_clamp_ms": DAY_MS,
        "yield_points": [],
        "expected_duration_ms": E,
        "cooldown_ms": 1000,
        "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
        "strategic_weight_milli": 1000,
        "instance_overrides": [],
        "benefit_judged": true,
        "benefit_fixed_value_milli": value
    })
}

/// A pool of `instance-a` that does not regenerate, of capacity 1 000 000, observed by
/// `observation`.
fn declared_pool(id: &str, observation: Value) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "instance", "instance_id": A},
        "capacity": 1_000_000,
        "projection": {"kind": "none"},
        "observation": observation,
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000}
    })
}

/// A pool of `instance-a` without a valuation, observed by `observation`, writing [`DEFAULTS`] and
/// [`TIERS`].
fn goal_pool(id: &str, observation: Value) -> Value {
    let mut pool = declared_pool(id, observation);
    pool["target_defaults"] = json!({"scale": DEFAULTS.0, "importance_milli": DEFAULTS.1});
    pool["goal_tiers"] = json!({"need_importance_milli": TIERS.0, "stock_cap_milli": TIERS.1});
    pool
}

/// The capacity fact plus `offset`.
fn line(offset: i64) -> Value {
    json!({"type": "fact", "fact_key": CAPACITY_KEY, "offset": offset})
}

/// The fixture's pools: the capped pool, `pool.tiered` and `pool.listed`.
fn pools() -> Value {
    let mut capped = declared_pool(POOL, json!({"kind": "fact", "fact_key": READING_KEY}));
    capped["valuation"] =
        json!({"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000});
    capped["capacity"] = line(0);
    capped["stop_line"] = line(-40);
    capped["hard_line"] = line(-900);
    let listed = json!({
        "kind": "list_rows",
        "fact_key": LIST_KEY,
        "key_column": "id",
        "value_column": "count",
        "rows": [{"key": "item_a", "factor": 1}]
    });
    json!([
        capped,
        goal_pool(TIERED, json!({"kind": "fact", "fact_key": TIERED_KEY})),
        goal_pool(LISTED, listed)
    ])
}

fn catalog(tasks: Vec<Value>) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.455-rr3e",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-455-rr3e"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": SCHEDULING_SCHEMA_VERSION_V2,
            "catalog": header,
        });
        document[member] = entries;
        document
    };
    let profile = json!({
        "id": "fixture-activity",
        "scope": {"kind": "game", "game_id": GAME},
        "windows": [{
            "weekdays": [1, 2, 3, 4, 5, 6, 7],
            "utc_offset_minutes": 0,
            "start_minute_of_day": 0,
            "end_minute_of_day": 0
        }],
        "daily_budget": 24,
        "max_window_iterations": 4,
        "session_max_ms": 7200000,
        "detection_budget": {
            "window_dispatch_limit": 4,
            "window_runtime_ms": 300000,
            "expected_duration_ms": 60000
        },
        "minimum_interval_ms": 60000,
        "maximum_interval_ms": 300000,
        "seed_source": "ledger",
        "resample_policy": "same_round_stable",
        "importance_milli": 1000,
        "goals": []
    });
    compile_catalog(&CatalogSources {
        tasks: source("tasks", &document("tasks", Value::Array(tasks))),
        pools: source("pools", &document("pools", pools())),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the catalog compiles: {:?}", failure.diagnostics()))
}

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture-455-rr3e/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}
