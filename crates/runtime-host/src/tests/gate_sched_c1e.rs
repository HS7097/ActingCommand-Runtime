// SPDX-License-Identifier: AGPL-3.0-only

//! #428 SCHED-C1e (register §1.299 SC1-12; the coordinator's point 8 and answers 3, 5 and 8 of
//! 11-11, not Alice's words; Alice, 11-09 17:42: 「这个红线会随着时间由外部修改」; coordinator work
//! order `rd-1111/brief-sched-c1e.md`): one instance's pool lines are set and cleared while the
//! Runtime runs through `ConfigureSettings`, on `instance.<instance id>.scheduling.pool_lines`.
//! - H1: on a connection bound to `cli`, the instruction completes with its records: the instance's
//!   `config.parameters.instance` row (`instruction`) and the `host.config_overrides` row with its
//!   `channel`. A pool the active generation does not give the instance, or gives without a
//!   declared valuation, is refused `config_value_invalid` (`section_check`, the instance's key)
//!   and nothing changes. A later generation that no longer values the pool leaves the value in
//!   effect, a new instruction naming it is refused, and a restart restores the value.
//!
//! The Runtime starts on fakes over one state root with the fixture catalog activated; the
//! evaluator's reading of the lines is `actingcommand-policy`'s `gate_sched_c1e`.

use super::*;
use actingcommand_contract::{
    CONFIG_OVERRIDES_FACT_KEY, ConfigParameterSource, ConfigSettingChange, FactScalar,
    RuntimeChannel, RuntimeFactScope, config_parameters_fact_key,
};
use actingcommand_policy::SCHEDULING_SCHEMA_VERSION_V2;
use actingcommand_runtime_config::scheduling::INSTANCE_POOL_LINES;
use actingcommand_runtime_config::{
    OVERRIDE_BASE_JSON, OVERRIDE_BASE_SOURCE, OVERRIDE_CHANNEL, OVERRIDE_VALUE_JSON,
    canonical_json, instance_key,
};
use serde_json::{Value, json};

/// The fixture catalog's game, its capped pool, a pool it gives without a valuation, its produced
/// pool and its one consumer.
const GAME: &str = "fixture-game-a";
const POOL: &str = "pool.neutral";
const PLAIN: &str = "pool.plain";
const OUT: &str = "pool.out";
const TASK: &str = "fixture.capped";

/// A start of instance A over `root`, its configuration naming the instance.
fn start(root: &TempDir, instance: InstanceId) -> RuntimeHost {
    RuntimeHost::start(
        config(root).with_config_manifest(day_boundary_off_manifest(&instance)),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host")
}

/// A pool of instance A read from `resource.<the id without "pool.">`, of constant `capacity`,
/// valued 1000 milli a unit when `valued`.
fn pool(id: &str, capacity: u64, valued: bool) -> Value {
    let mut pool = json!({
        "id": id,
        "scope": {"kind": "instance", "instance_id": POLICY_INSTANCE_ALIAS},
        "capacity": capacity,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": format!("resource.{}", &id["pool.".len()..])},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000}
    });
    if valued {
        pool["valuation"] =
            json!({"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000});
    }
    pool
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

/// Version `version` of a V2 catalog of the game: the capped pool (capacity 180, its lines 140 and
/// 104), valued when `capped_valued`; the produced pool; a pool of no valuation; and one judged
/// consumer of 30 units a run.
fn sources(version: u64, capped_valued: bool) -> CatalogSources {
    let header = json!({
        "catalog_id": "fixture.sched-c1e",
        "catalog_version": version,
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
    let mut capped = pool(POOL, 180, capped_valued);
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
            format!("memory://fixture-sched-c1e/{name}.json"),
            serde_json::to_vec(&document).expect("fixture document bytes"),
        )
    };
    CatalogSources {
        tasks: source("tasks", document("tasks", json!([task]))),
        pools: source(
            "pools",
            document(
                "pools",
                json!([pool(OUT, 1_000_000, true), capped, pool(PLAIN, 50, false)]),
            ),
        ),
        activity: source("activity", document("profiles", json!([profile]))),
        timeline: source("timeline", document("events", json!([]))),
        selection: None,
    }
}

/// A's `instance.<instance id>.scheduling.pool_lines`.
fn key(instance: InstanceId) -> String {
    instance_key(
        &crate::failure_identity::identifier_text(&instance),
        INSTANCE_POOL_LINES,
    )
}

/// Lines of `pool` for the instruction.
fn lines(pool: &str) -> Value {
    json!({pool: {"stop_line": 150, "hard_line": 110}})
}

/// The `ConfigureSettings` that sets A's lines to `value`, on `client`'s connection: the request
/// and its receipt.
fn configure(
    client: &mut TestClient,
    instance: InstanceId,
    value: Value,
) -> (RuntimeRequest, RuntimeReceipt) {
    let request = client.request(RuntimeOperation::ConfigureSettings {
        changes: vec![ConfigSettingChange {
            key: key(instance),
            value: Some(value),
        }],
        expected_version: None,
    });
    let receipt = client.send(&request);
    (request, receipt)
}

/// The rows of the runtime fact `key` the host holds; none when it holds no such fact.
fn fact_rows(host: &RuntimeHost, key: &str) -> Option<Vec<BTreeMap<String, FactScalar>>> {
    host.runtime_fact_snapshot()
        .expect("the runtime facts")
        .records
        .into_iter()
        .find(|record| record.scope == RuntimeFactScope::Runtime && record.key == key)
        .map(|record| match record.value {
            ContractFactValue::RecordList(rows) => rows,
            other => panic!("{key} is a record list: {other:?}"),
        })
}

/// The `config.parameters.instance` row of `key`: its value and source.
fn instance_row(host: &RuntimeHost, key: &str) -> Option<(FactScalar, FactScalar)> {
    fact_rows(host, &config_parameters_fact_key("instance"))
        .expect("config.parameters.instance")
        .into_iter()
        .find(|row| row.get("key") == Some(&FactScalar::String(key.to_owned())))
        .map(|row| {
            (
                row.get("value").cloned().expect("the row's value"),
                row.get("source").cloned().expect("the row's source"),
            )
        })
}

/// The `host.config_overrides` row of `key`.
fn override_row(host: &RuntimeHost, key: &str) -> Option<BTreeMap<String, FactScalar>> {
    fact_rows(host, CONFIG_OVERRIDES_FACT_KEY)?
        .into_iter()
        .find(|row| row.get("config_key") == Some(&FactScalar::String(key.to_owned())))
}

fn text(value: &str) -> FactScalar {
    FactScalar::String(value.to_owned())
}

/// The refusal of `request`: Denied, `config_value_invalid` with `section_check` for `key`.
fn refused(host: &RuntimeHost, request: &RuntimeRequest, receipt: &RuntimeReceipt, key: &str) {
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    let outcome = host
        .failure_outcome_for_test(request.request_id())
        .expect("refusal outcomes")
        .expect("the request was refused");
    assert_eq!(outcome.code().as_str(), "config_value_invalid");
    assert_eq!(outcome.value("config_key"), Some(&json!(key)));
    assert_eq!(
        outcome.value("config_value_problem"),
        Some(&json!("section_check"))
    );
}

// H1: the instruction's records through `ConfigureSettings` on a bound connection, the active
// generation's valued pools, and a restart's restore.
#[test]
fn sched_c1e_h1_an_instruction_sets_an_instances_lines_and_a_restart_keeps_them() {
    let root = TempDir::new().expect("tempdir");
    let instance = instance_id();
    let key = key(instance);
    let host = start(&root, instance);
    host.activate_policy_catalog(&sources(1, true))
        .expect("activate the fixture catalog");
    assert_eq!(instance_row(&host, &key), None, "the file names none");
    let mut client = TestClient::connect(&host);
    let bind = client.request(RuntimeOperation::BindChannel {
        channel: RuntimeChannel::Cli,
        via: None,
    });
    assert_eq!(client.send(&bind).state(), RuntimeReceiptState::Completed);

    let value = lines(POOL);
    let (_, receipt) = configure(&mut client, instance, value.clone());
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let Some(RuntimeResult::SettingsConfigured { settings, .. }) = receipt.result() else {
        panic!("a settings receipt: {receipt:?}");
    };
    let [setting] = settings.as_slice() else {
        panic!("one setting: {receipt:?}");
    };
    assert_eq!(setting.key, key);
    assert_eq!(setting.source, ConfigParameterSource::Instruction);
    let canonical = canonical_json(&value);
    assert_eq!(
        instance_row(&host, &key),
        Some((text(&canonical), text("instruction")))
    );
    let row = override_row(&host, &key).expect("the instruction's row");
    assert_eq!(row.get(OVERRIDE_VALUE_JSON), Some(&text(&canonical)));
    assert_eq!(row.get(OVERRIDE_BASE_JSON), Some(&text("null")));
    assert_eq!(row.get(OVERRIDE_BASE_SOURCE), Some(&text("default")));
    assert_eq!(row.get(OVERRIDE_CHANNEL), Some(&text("cli")));

    for pool in [PLAIN, "pool.gone"] {
        let (request, receipt) = configure(&mut client, instance, lines(pool));
        refused(&host, &request, &receipt, &key);
        assert_eq!(
            override_row(&host, &key).and_then(|row| row.get(OVERRIDE_VALUE_JSON).cloned()),
            Some(text(&canonical)),
            "{pool}: nothing changed"
        );
    }

    host.activate_policy_catalog(&sources(2, false))
        .expect("activate a generation that no longer values the pool");
    assert_eq!(
        instance_row(&host, &key),
        Some((text(&canonical), text("instruction"))),
        "the value stays in effect"
    );
    let (request, receipt) = configure(&mut client, instance, lines(POOL));
    refused(&host, &request, &receipt, &key);
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close the first start");

    let restarted = start(&root, instance);
    assert!(restarted.fatal_error().expect("runtime health").is_none());
    assert_eq!(
        instance_row(&restarted, &key),
        Some((text(&canonical), text("instruction"))),
        "restored at the start"
    );
    assert_eq!(
        override_row(&restarted, &key).and_then(|row| row.get(OVERRIDE_CHANNEL).cloned()),
        Some(text("cli"))
    );
    restarted.close().expect("close the restart");
}
