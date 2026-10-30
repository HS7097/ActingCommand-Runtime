// SPDX-License-Identifier: AGPL-3.0-only

// SS-2c (register §1.227; brief `rd-1029/brief-ss2c.md` tests 3, 5 and 6;
// contracts/standard-pack-schedule.md, "Judged rules", "Two priority tiers" and "Tools"):
// `actinglab standard-pack check` and `package bundle --schedule` count a rule that declares
// itself maintenance (`"maintenance": true`) as not judged and count the declared rules apart;
// with exactly two priority values they warn once per rule in the other tier than its declaration
// says (`standard_pack_priority_tier_mismatch`), after the undeclared-value warnings; a
// declaration of another shape is refused as `standard_pack_root_invalid`, `schedule_invalid` at
// the member. Neutral fixtures only: game `game-a`, packages on server `server-a`.

use crate::standard_pack_schedule::report_schedule;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{
    LabErrorClass, StandardPackRoot, StandardPackRootBytes, read_standard_pack_root,
};
use actingcommand_policy::BenefitCoverage;
use serde_json::{Value, json};
use std::path::Path;

const GAME: &str = "game-a";
const SERVER: &str = "server-a";
const VALUED: &str = "pool.valued";
/// A package nothing schedules: one `standard_pack_package_unclassified` warning first.
const UNSCHEDULED: &str = "fixture.unscheduled";

#[test]
fn ss2c_t5_t6_the_check_counts_declared_rules_apart_and_warns_of_a_tier_mismatch() {
    let rules = [
        // Declared, on the higher value, declaring no value: not judged, not warned about.
        ("fixture.declared", 200, Some(json!(true)), Vec::new()),
        // A maintenance task that forgot to declare: judged, undeclared, on the higher value.
        ("fixture.forgotten", 200, None, Vec::new()),
        // A business rule on the lower value with its value: complete.
        ("fixture.business", 100, None, vec![produce(VALUED, 1)]),
    ];
    let root = root(&rules.iter().map(|(id, ..)| *id).collect::<Vec<_>>());
    let path = Path::new("schedule.json");

    let report = report_schedule(GAME, &root, &[], &schedule(&rules), path)
        .unwrap_or_else(|error| panic!("the schedule is checked: {error}"));
    assert_eq!(
        report.benefit_coverage,
        BenefitCoverage {
            complete: 1,
            pending: 0,
            undeclared: 1,
            maintenance_declared: 1,
        }
    );
    let found = report
        .warnings
        .iter()
        .map(|warning| {
            (
                warning.code().as_str().to_owned(),
                warning.value("task_id").cloned(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        found,
        [
            (
                SharedCode::StandardPackPackageUnclassified
                    .as_str()
                    .to_owned(),
                None
            ),
            (
                SharedCode::StandardPackBenefitUndeclared
                    .as_str()
                    .to_owned(),
                Some(json!("fixture.forgotten"))
            ),
            (
                SharedCode::StandardPackPriorityTierMismatch
                    .as_str()
                    .to_owned(),
                Some(json!("fixture.forgotten"))
            ),
        ]
    );
    assert_eq!(report.warnings[2].value("game"), Some(&json!(GAME)));
}

#[test]
fn ss2c_t3_the_check_refuses_a_declaration_of_another_shape() {
    for value in [json!(false), json!("true"), json!(0), Value::Null] {
        let rules = [("fixture.declared", 100, Some(value.clone()), Vec::new())];
        let root = root(&["fixture.declared"]);
        let error = report_schedule(
            GAME,
            &root,
            &[],
            &schedule(&rules),
            Path::new("schedule.json"),
        )
        .err()
        .unwrap_or_else(|| panic!("{value}: refused"));
        assert_eq!(
            error.code,
            SharedCode::StandardPackRootInvalid.as_str(),
            "{value}: {}",
            error.message
        );
        assert_eq!(error.class, LabErrorClass::UsageValidation, "{value}");
        let outcome = error
            .details
            .as_ref()
            .and_then(|details| details.get("outcome"))
            .unwrap_or_else(|| panic!("{value}: no details.outcome: {}", error.message));
        assert_eq!(
            outcome["values"]["standard_pack_problem"], "schedule_invalid",
            "{value}: {outcome}"
        );
        assert_eq!(
            outcome["values"]["json_pointer"], "/rules/0/maintenance",
            "{value}: {outcome}"
        );
    }
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

fn produce(pool: &str, amount: u64) -> Value {
    json!({
        "pool_id": pool,
        "direction": "produce",
        "amount": amount,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    })
}

/// `schedule.json`: one rule per (package, priority, `maintenance` as written, produces), each on
/// an hourly clock; the valued pool.
fn schedule(rules: &[(&str, i64, Option<Value>, Vec<Value>)]) -> Vec<u8> {
    let rules = rules
        .iter()
        .map(|(id, priority, maintenance, produces)| {
            let mut rule = json!({
                "id": id,
                "package_id": id,
                "priority": priority,
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
            });
            if let Some(maintenance) = maintenance {
                rule["maintenance"] = maintenance.clone();
            }
            rule
        })
        .collect::<Vec<_>>();
    let valuation =
        json!({"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000});
    serde_json::to_vec(&json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": [],
        "rules": rules,
        "pools": [{
            "id": VALUED,
            "capacity": 1000,
            "projection": {"kind": "none"},
            "observation": {"kind": "fact", "fact_key": "resource.valued"},
            "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
            "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
            "valuation": valuation
        }],
        "activity": [],
        "timeline": [],
        "held": []
    }))
    .expect("schedule bytes")
}
