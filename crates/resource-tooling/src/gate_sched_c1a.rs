// SPDX-License-Identifier: AGPL-3.0-only

// #428 SCHED-C1a (register §1.299; coordinator work order `rd-1111/brief-sched-c1a.md`;
// contracts/standard-pack-schedule.md, "Judged rules", "The fixed value", "Provenance" and
// "Tools"): `actinglab standard-pack check` and `package bundle --schedule` count the judged rules
// that write a fixed value (`benefit_coverage.fixed_value`), each also counted as complete by the
// pools it consumes alone, and answer `benefit_provenance`: which rules run on a fixed value and
// which pools on a declared valuation block, each by who wrote it, `pack` (no origin member) or
// `model`, in file order. Neutral fixtures only: game `game-a`, packages on server `server-a`.

use crate::standard_pack_schedule::{readable_root, report_schedule};
use actingcommand_contract::{StandardPackRoot, StandardPackRootBytes, read_standard_pack_root};
use actingcommand_policy::{BENEFIT_FIXED_VALUE_SUBSYSTEM, BenefitCoverage};
use serde_json::{Value, json};
use std::path::Path;

const GAME: &str = "game-a";
const SERVER: &str = "server-a";
/// A pool whose valuation the pack's own hand wrote.
const VALUED: &str = "pool.valued";
/// A pool whose valuation the model's export wrote.
const MODELLED: &str = "pool.modelled";
/// A pool without a valuation.
const BARE: &str = "pool.bare";
/// A package nothing schedules: one `standard_pack_package_unclassified` warning.
const UNSCHEDULED: &str = "fixture.unscheduled";

// A file with a value the pack wrote (a collect rule that consumes nothing), a value the model's
// export marked (a consumer of the model-valued pool) and a rule judged by its effects; the pack
// declares one pool's valuation, the model another's, and a third has none. The three judged rules
// are complete, two of them by their fixed value; the provenance lists each rule and each valued
// pool by its origin, the bare pool in neither list.
#[test]
fn sched_c1a_t9_the_check_answers_coverage_and_provenance() {
    let mut by_pack = rule("fixture.pack", Vec::new(), Vec::new());
    by_pack["fixed_value_milli"] = json!(12_000);
    let mut by_model = rule(
        "fixture.model",
        vec![effect(MODELLED, "consume", 2)],
        Vec::new(),
    );
    by_model["fixed_value_milli"] = json!(8_000);
    by_model["fixed_value_origin"] = json!("model");
    let by_effects = rule(
        "fixture.effects",
        Vec::new(),
        vec![effect(VALUED, "produce", 1)],
    );
    let rules = [by_pack, by_model, by_effects];
    let packages = rules
        .iter()
        .map(|rule| rule["id"].as_str().expect("an id"))
        .collect::<Vec<_>>();
    let root = root(&packages);
    let mut modelled = pool(MODELLED, true);
    modelled["valuation_origin"] = json!("model");
    let schedule = serde_json::to_vec(&json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": [BENEFIT_FIXED_VALUE_SUBSYSTEM],
        "rules": rules,
        "pools": [pool(VALUED, true), modelled, pool(BARE, false)],
        "activity": [],
        "timeline": [],
        "held": []
    }))
    .expect("schedule bytes");

    // Workflow #455 G-R2s: every package of the root reads, none irreversible.
    let report = report_schedule(
        GAME,
        &root,
        &[],
        &schedule,
        Path::new("schedule.json"),
        &readable_root(&root),
    )
    .unwrap_or_else(|error| panic!("the schedule is checked: {error}"));
    assert_eq!(
        report.benefit_coverage,
        BenefitCoverage {
            complete: 3,
            pending: 0,
            undeclared: 0,
            maintenance_declared: 0,
            realization: 0,
            fixed_value: 2,
            // #428 COV-1 (register §1.299): no rule rides with a chain.
            follower: 0,
        }
    );
    assert_eq!(
        serde_json::to_value(&report.benefit_provenance).expect("the provenance"),
        json!({
            "fixed_value": {"pack": ["fixture.pack"], "model": ["fixture.model"]},
            "valuation": {"pack": [VALUED], "model": [MODELLED]}
        })
    );
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
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

fn effect(pool: &str, direction: &str, amount: u64) -> Value {
    json!({
        "pool_id": pool,
        "direction": direction,
        "amount": amount,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    })
}

/// A pool of constant capacity 1000 read from `resource.<the id without "pool.">`; with `valued`,
/// valued 1000 milli a unit.
fn pool(id: &str, valued: bool) -> Value {
    let mut pool = json!({
        "id": id,
        "capacity": 1000,
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

/// A `schedule.json` rule running the package of its own id on an hourly clock.
fn rule(id: &str, consumes: Vec<Value>, produces: Vec<Value>) -> Value {
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
        "consumes": consumes,
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
}
