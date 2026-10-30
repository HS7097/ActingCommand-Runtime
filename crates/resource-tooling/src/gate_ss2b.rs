// SPDX-License-Identifier: AGPL-3.0-only

// SS-2b (register §1.225; Alice's ruling 116, 「业务覆盖完整」; contracts/standard-pack-schedule.md,
// "Judged rules" and "Tools"): `actinglab standard-pack check` and `package bundle --schedule`
// report, per game, how many judged rules (their package not maintenance-role) have complete
// values, how many are pending and how many declare none, and warn once per undeclared rule
// (`standard_pack_benefit_undeclared`), after the composition's own warnings. Nothing is refused.
// Neutral fixtures only: game `game-a`, packages on server `server-a`.

use crate::standard_pack_schedule::{check_schedule, report_schedule};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{StandardPackRoot, StandardPackRootBytes, read_standard_pack_root};
use actingcommand_policy::BenefitCoverage;
use serde_json::{Value, json};
use std::path::Path;

const GAME: &str = "game-a";
const SERVER: &str = "server-a";
const VALUED: &str = "pool.valued";
const BARE: &str = "pool.bare";
/// A package nothing schedules: one `standard_pack_package_unclassified` warning first.
const UNSCHEDULED: &str = "fixture.unscheduled";

#[test]
fn ss2b_t9_the_check_counts_the_judged_rules_and_warns_once_per_undeclared_rule() {
    let rules = [
        ("fixture.complete", vec![produce(VALUED, json!(1))]),
        ("fixture.pending", vec![produce(VALUED, json!("pending"))]),
        ("fixture.none", Vec::new()),
        ("fixture.bare", vec![produce(BARE, json!(1))]),
    ];
    let root = root(&rules.iter().map(|(id, _)| *id).collect::<Vec<_>>());
    let schedule = schedule(&rules);
    let path = Path::new("schedule.json");

    let report = report_schedule(GAME, &root, &[], &schedule, path)
        .unwrap_or_else(|error| panic!("the schedule is checked: {error}"));
    assert_eq!(
        report.benefit_coverage,
        BenefitCoverage {
            complete: 1,
            pending: 1,
            undeclared: 2,
            // SS-2c (register §1.227): the count of rules declaring themselves maintenance.
            maintenance_declared: 0,
        }
    );
    let codes = report
        .warnings
        .iter()
        .map(|warning| warning.code().as_str().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        codes,
        [
            SharedCode::StandardPackPackageUnclassified.as_str(),
            SharedCode::StandardPackBenefitUndeclared.as_str(),
            SharedCode::StandardPackBenefitUndeclared.as_str(),
        ]
    );
    for (warning, (rule_id, pool_id)) in report.warnings[1..]
        .iter()
        .zip([("fixture.none", None), ("fixture.bare", Some(BARE))])
    {
        assert_eq!(warning.value("game"), Some(&json!(GAME)), "{warning:?}");
        assert_eq!(
            warning.value("task_id"),
            Some(&json!(rule_id)),
            "{warning:?}"
        );
        assert_eq!(
            warning.value("pool_id"),
            pool_id.map(|pool| json!(pool)).as_ref(),
            "{warning:?}"
        );
    }

    // The composition's own warnings stay as they were.
    let warnings = check_schedule(GAME, &root, &[], &schedule, path)
        .unwrap_or_else(|error| panic!("the schedule is checked: {error}"));
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].is(SharedCode::StandardPackPackageUnclassified));
}

/// The game root: a bundle index v4 with a package per rule, and one package nothing schedules.
fn root(packages: &[&str]) -> StandardPackRoot {
    let packs = packages
        .iter()
        .copied()
        .chain([UNSCHEDULED])
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

fn produce(pool: &str, amount: Value) -> Value {
    json!({
        "pool_id": pool,
        "direction": "produce",
        "amount": amount,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    })
}

fn pool(id: &str, valuation: Option<Value>) -> Value {
    let mut pool = json!({
        "id": id,
        "capacity": 1000,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": format!("resource.{}", &id["pool.".len()..])},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000}
    });
    if let Some(valuation) = valuation {
        pool["valuation"] = valuation;
    }
    pool
}

/// `schedule.json`: one rule per (package, produces), each on an hourly clock; the valued pool and
/// the bare one (no valuation).
fn schedule(rules: &[(&str, Vec<Value>)]) -> Vec<u8> {
    let rules = rules
        .iter()
        .map(|(id, produces)| {
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
                "consumes": [],
                "produces": produces,
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
        })
        .collect::<Vec<_>>();
    let valuation =
        json!({"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000});
    serde_json::to_vec(&json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": [],
        "rules": rules,
        "pools": [pool(VALUED, Some(valuation)), pool(BARE, None)],
        "activity": [],
        "timeline": [],
        "held": []
    }))
    .expect("schedule bytes")
}
