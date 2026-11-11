// SPDX-License-Identifier: AGPL-3.0-only

//! #428 SCHED-C1d (register §1.299 SC1-11; the coordinator's correction of 11-11 13:4x, not
//! Alice's words: iron rule 13 and Alice's rulings of 10-09 20:2x and 20:5x keep the episodes as a
//! tagged program fact, not a state file; coordinator work order `rd-1111/brief-sched-c1d.md`):
//! the capped spending order's episodes are the tagged program fact `host.capped_episodes`, which a
//! start loads from the ledger's tagged records before the first evaluation, by the anchor rule of
//! `host.idle_state` (#405 N-2). Each test starts the Runtime on fakes over one state root, on a
//! manual Runtime clock, and starts it again on the same root; the episodes the first start keeps
//! are a record of the fact written through the host's one writer. The evaluator's own use of a
//! restored episode is `actingcommand-policy`'s `gate_sched_c1d`.
//! - H1: a start keeps the episodes the last start that completed its settlement recorded,
//!   records them again under its own anchor, and the next start keeps them again.
//! - H2: a value the start cannot keep (not of the shape, against its rules, stale by the anchor
//!   rule, naming a pool the active catalog does not have or an instance that is not registered)
//!   is one Warning `capped_state_failed` (`operation` `load_capped_episodes`); the start goes on
//!   with no episode of it.
//! - H3: a record the fact store refuses is the warning with `operation` `store_capped_episodes`
//!   and the refusal as its cause; the start goes on with the episodes in memory.
//! - H4: a Runtime that never has an episode records no such fact.

use super::*;
use crate::codes::{CappedStateCode, CappedStateLocation};
use crate::host::CAPPED_EPISODES_STORE_REFUSED_FOR_TEST;
use actingcommand_contract::codes::SanitizationCode;
use actingcommand_contract::outcome::{CauseRelation, Outcome};
use actingcommand_contract::{FactScalar, RuntimeFactRecord, RuntimeFactScope, RuntimePayload};
use actingcommand_policy::{
    KeptConsumer, KeptEpisode, KeptMember, KeptRound, SCHEDULING_SCHEMA_VERSION_V2,
};
use serde_json::{Value, json};

/// The fact, and the anchor fact its writer row is read against.
const FACT: &str = "host.capped_episodes";
const SETTLEMENT: &str = "host.contained_settlement";
/// The first start's Runtime clock, and the restarts' an hour and two hours later.
const FIRST_START_UNIX_MS: u64 = POLICY_NOW_UNIX_MS;
const RESTART_UNIX_MS: u64 = POLICY_NOW_UNIX_MS + 3_600_000;
const SECOND_RESTART_UNIX_MS: u64 = POLICY_NOW_UNIX_MS + 7_200_000;
/// The fixture catalog's game, its capped pool and its one consumer.
const GAME: &str = "fixture-game-a";
const POOL: &str = "pool.neutral";
const OUT: &str = "pool.out";
const TASK: &str = "fixture.capped";
/// A pool and an instance alias the fixture does not have.
const POOL_GONE: &str = "pool.gone";
const INSTANCE_GONE: &str = "fixture-instance-gone";

/// A start of instance A over `root` with its Runtime clock at `clock_unix_ms`.
fn start_at(
    root: &TempDir,
    clock_unix_ms: u64,
    instance: InstanceId,
) -> RuntimeHostResult<RuntimeHost> {
    let clock = Arc::new(ManualRuntimeClock::new(clock_unix_ms, 0));
    RuntimeHost::start(
        config(root).with_runtime_clock(clock),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance,
            Arc::new(FakeState::default()),
        )),
    )
}

fn effect(pool: &str, direction: &str, amount: u64) -> Value {
    json!({
        "pool_id": pool,
        "direction": direction,
        "amount": amount,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    })
}

/// A pool of instance A that does not regenerate, read from `resource.<the id without
/// "pool.">`, of constant `capacity`, valued 1000 milli a unit.
fn valued(id: &str, capacity: u64) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "instance", "instance_id": POLICY_INSTANCE_ALIAS},
        "capacity": capacity,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": format!("resource.{}", &id["pool.".len()..])},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
        "valuation": {"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000}
    })
}

/// A V2 catalog of the game: the capped pool (capacity 180, its stop line 140 and its hard line
/// 104), the produced pool, and one judged consumer of 30 units a run.
fn capped_sources() -> CatalogSources {
    let header = json!({
        "catalog_id": "fixture.sched-c1d",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-a"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": SCHEDULING_SCHEMA_VERSION_V2,
            "catalog": header,
        });
        document[member] = entries;
        document
    };
    let mut capped = valued(POOL, 180);
    capped["stop_line"] = json!(140);
    capped["hard_line"] = json!(104);
    let task = json!({
        "id": TASK,
        "scope": {"kind": "game", "game_id": GAME},
        "entrypoint": {"operation_id": "operation.observe"},
        "procedure_ref": "procedure.observe",
        "priority": 100,
        "trigger": {
            "kind": "clock",
            "schedule": {
                "kind": "interval",
                "clock_source": {"kind": "local"},
                "every_ms": 3_600_000,
                "anchor_ms": 0
            }
        },
        "feedback_stop": {
            "kind": "outcome",
            "task_id": TASK,
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": [effect(POOL, "consume", 30)],
        "produces": [effect(OUT, "produce", 30)],
        "on_failure": {
            "action": "continue",
            "retry_limit": 2,
            "retry_backoff_ms": 30000,
            "escalation_threshold": 3
        },
        "sensitive": false,
        "next_run_clamp_ms": 86_400_000,
        "yield_points": [],
        "expected_duration_ms": 60_000,
        "cooldown_ms": 1000,
        "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
        "strategic_weight_milli": 1000,
        "instance_overrides": [],
        "benefit_judged": true
    });
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
    let source = |name: &str, document: Value| {
        CatalogDocumentSource::new(
            format!("memory://fixture-sched-c1d/{name}.json"),
            serde_json::to_vec(&document).expect("fixture document bytes"),
        )
    };
    CatalogSources {
        tasks: source("tasks", document("tasks", json!([task]))),
        pools: source(
            "pools",
            document("pools", json!([valued(OUT, 1_000_000), capped])),
        ),
        activity: source("activity", document("profiles", json!([profile]))),
        timeline: source("timeline", document("events", json!([]))),
        selection: None,
    }
}

/// The start anchor `host.contained_settlement` holds: the start event of the last start that
/// completed its settlement.
fn anchor_of(host: &RuntimeHost) -> i64 {
    match host
        .runtime_fact_for_test(&RuntimeFactScope::Runtime, SETTLEMENT)
        .expect("the program facts")
        .map(|record| record.value)
    {
        Some(ContractFactValue::Integer(anchor)) => anchor,
        other => panic!("host.contained_settlement is the start anchor: {other:?}"),
    }
}

fn row(members: &[(&str, FactScalar)]) -> BTreeMap<String, FactScalar> {
    members
        .iter()
        .map(|(member, value)| ((*member).to_owned(), value.clone()))
        .collect()
}

fn text(value: &str) -> FactScalar {
    FactScalar::String(value.to_owned())
}

/// The rows of one episode of `pool` on `instance`, number `number` in its first round, begun at
/// the first start from 200 with the consumer alone in its order.
fn episode_rows(instance: &str, pool: &str, number: i64) -> Vec<BTreeMap<String, FactScalar>> {
    vec![
        row(&[
            ("instance_alias", text(instance)),
            ("pool_id", text(pool)),
            ("capped_episode", FactScalar::Integer(number)),
            ("capped_round", FactScalar::Integer(1)),
            (
                "round_started_at_unix_ms",
                FactScalar::TimestampMs(FIRST_START_UNIX_MS),
            ),
            ("pool_reading", FactScalar::Integer(200)),
        ]),
        row(&[
            ("consumer_task_id", text(TASK)),
            ("consumed", FactScalar::Integer(30)),
            ("limited", FactScalar::Boolean(false)),
        ]),
        row(&[
            ("member_task_id", text(TASK)),
            ("consumed", FactScalar::Integer(30)),
        ]),
    ]
}

/// The writer row naming `anchor`, then `rows`.
fn with_writer(
    anchor: i64,
    rows: Vec<BTreeMap<String, FactScalar>>,
) -> Vec<BTreeMap<String, FactScalar>> {
    let mut all = vec![row(&[("start_anchor", FactScalar::Integer(anchor))])];
    all.extend(rows);
    all
}

/// The episode [`episode_rows`] describes on instance A, as the policy host keeps it.
fn kept_episode() -> KeptEpisode {
    KeptEpisode {
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        pool_id: POOL.to_owned(),
        number: 2,
        rounds: 1,
        consumers: vec![KeptConsumer {
            task_id: TASK.to_owned(),
            consumed: 30,
            limited: false,
        }],
        round: Some(KeptRound {
            started_at_unix_ms: FIRST_START_UNIX_MS,
            reading: 200,
            order: vec![KeptMember {
                task_id: TASK.to_owned(),
                consumed: 30,
                passed: false,
                gate_code: None,
            }],
        }),
    }
}

/// The first start over `root`: the fixture catalog activated, and `host.capped_episodes`
/// recorded with `rows` under the writer `anchor + shift`, `anchor` this start's anchor, which it
/// returns; closed.
fn first_start(
    root: &TempDir,
    instance: InstanceId,
    shift: i64,
    rows: Vec<BTreeMap<String, FactScalar>>,
) -> i64 {
    let host = start_at(root, FIRST_START_UNIX_MS, instance).expect("the first start");
    host.activate_policy_catalog(&capped_sources())
        .expect("activate the fixture catalog");
    let anchor = anchor_of(&host);
    host.record_runtime_fact_for_test(RuntimeFactRecord {
        scope: RuntimeFactScope::Runtime,
        key: FACT.to_owned(),
        value: ContractFactValue::RecordList(with_writer(anchor + shift, rows)),
        observed_at_unix_ms: FIRST_START_UNIX_MS + 1,
        source: OriginModule::Runtime,
        ttl_ms: None,
    })
    .expect("the kept episodes");
    host.close().expect("close the first start");
    anchor
}

/// The rows of `host.capped_episodes` the host holds, its writer row first.
fn kept_rows(host: &RuntimeHost) -> Option<Vec<BTreeMap<String, FactScalar>>> {
    host.runtime_fact_for_test(&RuntimeFactScope::Runtime, FACT)
        .expect("the program facts")
        .map(|record| match record.value {
            ContractFactValue::RecordList(rows) => rows,
            other => panic!("host.capped_episodes is a record list: {other:?}"),
        })
}

/// The outcomes of the `capped_state_failed` records, in ledger order; each is a Warning.
fn capped_warnings(host: &RuntimeHost) -> Vec<Outcome> {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::RuntimeFailed),
        ..EventQuery::default()
    })
    .expect("the runtime failures")
    .into_iter()
    .filter_map(|event| match event.payload() {
        EventPayload::Runtime(RuntimePayload::Failed(record))
            if record.outcome().is(CappedStateCode::Failed) =>
        {
            assert_eq!(event.severity(), EventSeverity::Warning, "a Warning");
            Some(record.outcome().clone())
        }
        _ => None,
    })
    .collect()
}

/// H1: the first start recorded an episode of the capped pool on A (number 2, round 1) under its
/// own anchor. The restart keeps it: the policy host holds it as it was kept, nothing is
/// reported, and the fact is recorded again under the restart's anchor with the same episode rows.
/// A second restart keeps it again from that record.
#[test]
fn sched_c1d_h1_a_start_keeps_the_episodes_of_the_last_settled_start() {
    let root = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let first = first_start(
        &root,
        instance,
        0,
        episode_rows(POLICY_INSTANCE_ALIAS, POOL, 2),
    );

    let mut anchors = vec![first];
    for clock in [RESTART_UNIX_MS, SECOND_RESTART_UNIX_MS] {
        let host = start_at(&root, clock, instance).expect("the restart completes");
        assert!(host.fatal_error().expect("runtime health").is_none());
        let memory = host
            .capped_episodes_for_test()
            .expect("the capped episodes");
        assert_eq!(memory.episode(POLICY_INSTANCE_ALIAS, POOL), Some((2, 1)));
        assert_eq!(memory.kept(), [kept_episode()], "kept as it was recorded");
        assert!(capped_warnings(&host).is_empty(), "nothing is reported");
        let anchor = anchor_of(&host);
        assert_eq!(
            kept_rows(&host),
            Some(with_writer(
                anchor,
                episode_rows(POLICY_INSTANCE_ALIAS, POOL, 2)
            )),
            "recorded again under the start's own anchor"
        );
        anchors.push(anchor);
        host.close().expect("close the restart");
    }
    let distinct = anchors.iter().collect::<BTreeSet<_>>();
    assert_eq!(
        distinct.len(),
        anchors.len(),
        "each start records under its own anchor: {anchors:?}"
    );
}

/// H2: each value the start cannot keep is dropped with one Warning `capped_state_failed`
/// (`operation` `load_capped_episodes`, `fact_key` `host.capped_episodes`, and the episode's pair
/// when one episode is dropped); the restart completes with no host fault and no episode, and
/// records the fact again under its own anchor with no episode row.
#[test]
fn sched_c1d_h2_a_value_the_start_cannot_keep_warns_and_the_start_goes_on() {
    // Each case: the rows, the writer's distance from the first start's anchor, and the pair the
    // warning names.
    let cases = [
        (
            "a row of no kind",
            vec![row(&[("unknown_member", FactScalar::Integer(1))])],
            0,
            None,
        ),
        (
            "an episode number 0",
            episode_rows(POLICY_INSTANCE_ALIAS, POOL, 0),
            0,
            Some((POLICY_INSTANCE_ALIAS, POOL)),
        ),
        (
            "stale by the anchor rule",
            episode_rows(POLICY_INSTANCE_ALIAS, POOL, 2),
            1_000,
            None,
        ),
        (
            "a pool the catalog does not have",
            episode_rows(POLICY_INSTANCE_ALIAS, POOL_GONE, 2),
            0,
            Some((POLICY_INSTANCE_ALIAS, POOL_GONE)),
        ),
        (
            "an instance that is not registered",
            episode_rows(INSTANCE_GONE, POOL, 2),
            0,
            Some((INSTANCE_GONE, POOL)),
        ),
    ];
    for (case, rows, shift, pair) in cases {
        let root = TempDir::new().expect("tempdir");
        let instance = instance_id();
        first_start(&root, instance, shift, rows);
        let host = start_at(&root, RESTART_UNIX_MS, instance).expect("the restart completes");
        assert!(
            host.fatal_error().expect("runtime health").is_none(),
            "{case}"
        );
        assert!(
            host.capped_episodes_for_test()
                .expect("the capped episodes")
                .is_empty(),
            "{case}: no episode"
        );
        let warnings = capped_warnings(&host);
        assert_eq!(warnings.len(), 1, "{case}: {warnings:#?}");
        let warning = &warnings[0];
        assert_eq!(
            outcome_str(warning, "operation"),
            Some(CappedStateLocation::Load.as_str()),
            "{case}"
        );
        assert_eq!(outcome_str(warning, "fact_key"), Some(FACT), "{case}");
        assert_eq!(
            outcome_str(warning, "instance_id").zip(outcome_str(warning, "pool_id")),
            pair,
            "{case}: {warning:#?}"
        );
        assert_eq!(
            kept_rows(&host),
            Some(with_writer(anchor_of(&host), Vec::new())),
            "{case}: recorded again with no episode"
        );
        host.close().expect("close the restart");
    }
}

/// H3: the restart keeps the episode, and the fact store refuses its record of the fact under the
/// restart's anchor (a record list too large for the store, injected): one Warning
/// `capped_state_failed` with `operation` `store_capped_episodes` and the refusal as its
/// `caused_by` link. The restart completes with no host fault, holds the episode in memory, and the
/// fact keeps the first start's record.
#[test]
fn sched_c1d_h3_a_record_the_store_refuses_warns_and_the_runtime_goes_on() {
    let root = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let first = first_start(
        &root,
        instance,
        0,
        episode_rows(POLICY_INSTANCE_ALIAS, POOL, 2),
    );
    let recorded = with_writer(first, episode_rows(POLICY_INSTANCE_ALIAS, POOL, 2));

    CAPPED_EPISODES_STORE_REFUSED_FOR_TEST.with(|refused| refused.set(true));
    let host = start_at(&root, RESTART_UNIX_MS, instance).expect("the restart completes");
    assert!(
        !CAPPED_EPISODES_STORE_REFUSED_FOR_TEST.with(std::cell::Cell::get),
        "the start's record met the refusal"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    assert_eq!(
        host.capped_episodes_for_test()
            .expect("the capped episodes")
            .episode(POLICY_INSTANCE_ALIAS, POOL),
        Some((2, 1)),
        "the episode stays in memory"
    );
    let warnings = capped_warnings(&host);
    let stored = warnings
        .iter()
        .filter(|warning| {
            outcome_str(warning, "operation") == Some(CappedStateLocation::Store.as_str())
        })
        .collect::<Vec<_>>();
    assert_eq!(stored.len(), 1, "{warnings:#?}");
    assert_eq!(outcome_str(stored[0], "fact_key"), Some(FACT));
    assert!(
        outcome_links(stored[0], CauseRelation::CausedBy)
            .iter()
            .any(|refusal| refusal.is(SanitizationCode::RuntimeFactRecordListTooLarge)),
        "the refusal is the cause: {:#?}",
        stored[0]
    );
    assert_eq!(
        kept_rows(&host),
        Some(recorded),
        "the fact keeps the record the store did not replace"
    );
    host.close().expect("close the restart");
}

/// H4: a Runtime that never has an episode records no `host.capped_episodes`: the fixture
/// catalog-a has no valued pool, an evaluated cycle dispatches its task, and neither that start
/// nor the next records the fact or reports anything.
#[test]
fn sched_c1d_h4_a_runtime_that_never_has_an_episode_records_no_fact() {
    let root = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("the first start");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate catalog-a");
    let (cycle, _, _) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    assert!(cycle.evaluation.is_some(), "an evaluated cycle");
    assert_eq!(kept_rows(&host), None, "no fact");
    assert!(capped_warnings(&host).is_empty());
    host.close().expect("close the first start");

    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("the restart");
    assert_eq!(kept_rows(&host), None, "no fact after the restart");
    assert!(
        host.capped_episodes_for_test()
            .expect("the capped episodes")
            .is_empty()
    );
    assert!(capped_warnings(&host).is_empty());
    host.close().expect("close the restart");
}
