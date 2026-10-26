// SPDX-License-Identifier: AGPL-3.0-only

// #386 slice CF-4 (contracts/standard-pack-schedule.md, `requires`; contracts/scheduling/README.md,
// "Fact references"): `actinglab standard-pack check` and `package bundle --schedule` check a
// game root's `schedule.json` with the Runtime's composer, one bound instance per server. A rule
// whose `fact` predicate compares with another fact plus an offset passes when `requires` names
// `fact_reference`; without the entry the check reports it, `standard_pack_root_invalid` with
// `schedule_invalid` at the reference. Neutral fixtures only: game `game-a`, one package on
// server `server-a`.

use crate::standard_pack_schedule::check_schedule;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{
    LabErrorClass, StandardPackRoot, StandardPackRootBytes, read_standard_pack_root,
};
use serde_json::{Value, json};
use std::path::Path;

const GAME: &str = "game-a";
const SERVER: &str = "server-a";
const PACKAGE: &str = "fixture.amount";
const SUBSYSTEM: &str = "fact_reference";

#[test]
fn cf4_standard_pack_check_accepts_a_reference_its_requires_names_and_reports_one_it_does_not() {
    let root = root();
    let path = Path::new("schedule.json");

    let warnings = check_schedule(GAME, &root, &[], &schedule(true), path)
        .unwrap_or_else(|error| panic!("the schedule with its requires entry is checked: {error}"));
    assert!(warnings.is_empty(), "{warnings:?}");

    let error = check_schedule(GAME, &root, &[], &schedule(false), path)
        .expect_err("a reference without its requires entry is reported");
    assert_eq!(
        error.code,
        SharedCode::StandardPackRootInvalid.as_str(),
        "{}",
        error.message
    );
    assert_eq!(error.class, LabErrorClass::UsageValidation);
    let outcome = error
        .details
        .as_ref()
        .and_then(|details| details.get("outcome"))
        .unwrap_or_else(|| panic!("no details.outcome: {}", error.message));
    assert_eq!(
        outcome["values"]["standard_pack_problem"], "schedule_invalid",
        "{outcome}"
    );
    assert_eq!(
        outcome["values"]["json_pointer"], "/rules/0/trigger/value",
        "{outcome}"
    );
}

// The game root: a bundle index v4 with the one package.
fn root() -> StandardPackRoot {
    let bundle = serde_json::to_vec(&json!({
        "schema_version": "actingcommand.bundle.v4",
        "game": GAME,
        "packs": [{
            "package_id": PACKAGE,
            "server": SERVER,
            "entry_task_id": "task",
            "path": format!("packs/{PACKAGE}"),
            "file_count": 1,
            "byte_count": 64
        }]
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

// `schedule.json`: one rule running the package while `resource.neutral.amount` >=
// `resource.neutral.capacity` - 1000, with `requires` naming `fact_reference` when `declared`.
fn schedule(declared: bool) -> Vec<u8> {
    let requires = if declared {
        json!([SUBSYSTEM])
    } else {
        json!([])
    };
    let document: Value = json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": requires,
        "rules": [{
            "id": PACKAGE,
            "package_id": PACKAGE,
            "priority": 100,
            "trigger": {
                "kind": "fact",
                "fact_key": "resource.neutral.amount",
                "comparison": "greater_than_or_equal",
                "value": {"type": "fact", "fact_key": "resource.neutral.capacity", "offset": -1000},
                "max_age_ms": 600000
            },
            "feedback_stop": {
                "kind": "outcome",
                "task_id": PACKAGE,
                "outcome_key": "completed",
                "comparison": "eq",
                "value": {"type": "boolean", "value": true}
            },
            "consumes": [],
            "produces": [],
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
            "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
            "loop_budget": {"daily_limit": 24, "window_iteration_limit": 4, "max_runtime_ms": 300000},
            "strategic_weight_milli": 1000
        }]
    });
    serde_json::to_vec_pretty(&document).expect("schedule bytes")
}
