// SPDX-License-Identifier: AGPL-3.0-only

// #428 SCHED-C1f1 (register §1.303 SC1F-7; coordinator work order `rd-1111/brief-sched-c1f1.md`;
// the coordinator's rulings of 11-11 14:5x, not Alice's words; contracts/standard-pack-schedule.md,
// "A run's consumption read from a fact", "Tools"): `actinglab standard-pack check` and `package
// bundle --schedule` answer `consumption_facts`, one `{rule_id, pool_id, fact_key, fallback}` per
// `consumes` entry whose amount is read from a fact, in file order, empty when none, and count
// such an entry as a declared quantity in `benefit_coverage`. Neutral fixtures only: game
// `game-a`, packages on server `server-a`.

use crate::StandardPackCheckResponse;
use crate::standard_pack_schedule::{ScheduleReport, report_schedule};
use actingcommand_contract::{StandardPackRoot, StandardPackRootBytes, read_standard_pack_root};
use actingcommand_policy::{CONSUMPTION_FACT_SUBSYSTEM, ConsumptionFactRule};
use serde_json::{Value, json};
use std::path::Path;

const GAME: &str = "game-a";
const SERVER: &str = "server-a";
/// The consumed pool, valued.
const VALUED: &str = "pool.valued";
/// The produced pool, valued.
const OUT: &str = "pool.out";
/// The fact the first rule reads one run's amount of `VALUED` from.
const COST_KEY: &str = "resource.valued.cost";

// A file with a rule whose consumption of the valued pool is read from a fact (fallback 2), then a
// rule with a constant amount and a third reading its amount of the pool from another key
// (fallback 5): the check lists the two read from a fact in file order, with their keys and
// fallbacks, and counts all three rules complete. Its answer carries `consumption_facts` as listed;
// a file whose rules write no such amount answers an empty list.
#[test]
fn sched_c1f1_t7_the_check_lists_the_consumptions_read_from_a_fact() {
    let read =
        |key: &str, fallback: u64| json!({"type": "fact", "fact_key": key, "fallback": fallback});
    let rules = [
        rule("fixture.read", read(COST_KEY, 2)),
        rule("fixture.constant", json!(3)),
        rule("fixture.other", read("resource.valued.other", 5)),
    ];
    let report = check(&rules, &[CONSUMPTION_FACT_SUBSYSTEM]);
    assert_eq!(
        report.consumption_facts,
        [
            ConsumptionFactRule {
                rule_id: "fixture.read".to_owned(),
                pool_id: VALUED.to_owned(),
                fact_key: COST_KEY.to_owned(),
                fallback: 2,
            },
            ConsumptionFactRule {
                rule_id: "fixture.other".to_owned(),
                pool_id: VALUED.to_owned(),
                fact_key: "resource.valued.other".to_owned(),
                fallback: 5,
            },
        ]
    );
    assert_eq!(
        (
            report.benefit_coverage.complete,
            report.benefit_coverage.pending,
            report.benefit_coverage.undeclared
        ),
        (3, 0, 0)
    );
    let listed = answer(report);
    assert_eq!(
        listed["consumption_facts"],
        json!([
            {"rule_id": "fixture.read", "pool_id": VALUED, "fact_key": COST_KEY, "fallback": 2},
            {
                "rule_id": "fixture.other",
                "pool_id": VALUED,
                "fact_key": "resource.valued.other",
                "fallback": 5
            }
        ])
    );

    let plain = check(&[rule("fixture.constant", json!(3))], &[]);
    assert!(
        plain.consumption_facts.is_empty(),
        "{:?}",
        plain.consumption_facts
    );
    assert_eq!(answer(plain)["consumption_facts"], json!([]));
}

/// The check's answer carrying `report`, as `actinglab standard-pack check` writes it.
fn answer(report: ScheduleReport) -> Value {
    serde_json::to_value(StandardPackCheckResponse {
        status: "checked".to_owned(),
        root: "root".to_owned(),
        game: GAME.to_owned(),
        root_files: Vec::new(),
        pack_count: 0,
        warnings: report.warnings,
        benefit_coverage: Some(report.benefit_coverage),
        benefit_provenance: Some(report.benefit_provenance),
        benefit_followers: report.benefit_followers,
        consumption_facts: Some(report.consumption_facts),
        // Workflow #444 NOTICES-3: the answer's struct gained this member.
        notices: None,
        // Workflow #453 S8: the answer's struct gained this member.
        models: Vec::new(),
        // Workflow #463 V1: the answer's struct gained this member.
        variable_provenance: None,
    })
    .expect("the answer")
}

/// The report of `schedule.json` with `rules` and the two valued pools, naming `requires`,
/// against a root with a package per rule.
fn check(rules: &[Value], requires: &[&str]) -> ScheduleReport {
    let packages = rules
        .iter()
        .map(|rule| rule["package_id"].as_str().expect("a package id"))
        .collect::<Vec<_>>();
    let schedule = serde_json::to_vec(&json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": requires,
        "rules": rules,
        "pools": [pool(VALUED), pool(OUT)],
        "activity": [],
        "timeline": [],
        "held": []
    }))
    .expect("schedule bytes");
    report_schedule(
        GAME,
        &root(&packages),
        &[],
        &schedule,
        Path::new("schedule.json"),
    )
    .unwrap_or_else(|error| panic!("the schedule is checked: {error}"))
}

/// The game root: a bundle index v4 with a package per rule.
fn root(packages: &[&str]) -> StandardPackRoot {
    let packs = packages
        .iter()
        .map(|package_id| {
            json!({
                "package_id": package_id,
                "server": SERVER,
                "entry_task_id": "task",
                "path": format!("packs/{package_id}"),
                "file_count": 1,
                "byte_count": 64
            })
        })
        .collect::<Vec<_>>();
    let bundle = serde_json::to_vec(&json!({
        "schema_version": "actingcommand.bundle.v4",
        "game": GAME,
        "packs": packs
    }))
    .expect("bundle bytes");
    read_standard_pack_root(
        StandardPackRootBytes {
            bundle: Some(bundle.as_slice()),
            ..StandardPackRootBytes::default()
        },
        GAME,
    )
    .unwrap_or_else(|issue| panic!("the root reads: {issue}"))
    .expect("the root holds bundle.json")
}

/// A pool of constant capacity 1000 read from `resource.<the id without "pool.">`, valued 1000
/// milli a unit.
fn pool(id: &str) -> Value {
    json!({
        "id": id,
        "capacity": 1000,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": format!("resource.{}", &id["pool.".len()..])},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
        "valuation": {"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000}
    })
}

/// A `schedule.json` rule running the package of its own id on an hourly clock, consuming
/// `amount` of the valued pool a run and producing one unit of the other.
fn rule(id: &str, amount: Value) -> Value {
    json!({
        "id": id,
        "package_id": id,
        "priority": 100,
        "trigger": {
            "kind": "clock",
            "schedule": {
                "kind": "interval",
                "clock_source": {"kind": "local"},
                "every_ms": 3600000,
                "anchor_ms": 0
            }
        },
        "feedback_stop": {
            "kind": "outcome",
            "task_id": id,
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": [{
            "pool_id": VALUED,
            "direction": "consume",
            "amount": amount,
            "observation_source": "scan_verified",
            "confidence_milli": 1000
        }],
        "produces": [{
            "pool_id": OUT,
            "direction": "produce",
            "amount": 1,
            "observation_source": "scan_verified",
            "confidence_milli": 1000
        }],
        "on_failure": {
            "action": "continue",
            "retry_limit": 2,
            "retry_backoff_ms": 30000,
            "escalation_threshold": 3
        },
        "sensitive": false,
        "next_run_clamp_ms": 86400000,
        "yield_points": [],
        "expected_duration_ms": 60000,
        "cooldown_ms": 1000,
        "load_profile": {"kind": "light"},
        "strategic_weight_milli": 1000
    })
}
