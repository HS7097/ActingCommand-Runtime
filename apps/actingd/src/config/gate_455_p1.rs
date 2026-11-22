// SPDX-License-Identifier: AGPL-3.0-only

// #455 P1 (0.12.78; register §1.504; the coordinator's ruling 5 of 11-21 in its work order, not the
// owner's words; contracts/standard-pack-schedule.md, "`requires`"): this build's configuration
// manifest names no `pool_list_rows`, so the composer, given the subsystems the manifest names as
// the scheduler workbench and check-config give them, refuses a `schedule.json` that lists it in
// `requires`: `schedule_requires_missing` at that entry. The workbench keeps the game's last good
// rules on such a refusal, as on every refusal of a game's file (the host gate test
// `rl1b_a6_a_broken_file_keeps_that_games_last_good_rules_and_spares_the_others`). The names are
// made up.

use super::*;
use actingcommand_contract::{
    DeclaredCapabilities, IdentifierIssuer, IrreversiblePackages, StandardPackProblem,
    StandardPackRootBytes, read_standard_pack_root,
};
use actingcommand_policy::{
    POOL_LIST_ROWS_SUBSYSTEM, StandardPackBinding, StandardPackComposeInput, StandardPackGameFiles,
    compose_standard_pack_rules,
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use tempfile::TempDir;

const ALIAS: &str = "neutral.device";
const GAME: &str = "game-a";
const SERVER: &str = "server-a";
const PACKAGE: &str = "rule.plain";

fn configuration(root: &TempDir) -> ActingdConfigFile {
    let id = IdentifierIssuer::new()
        .expect("issuer")
        .mint_instance_id()
        .expect("instance id");
    let config = json!({
        "schema_version": CONFIG_SCHEMA_VERSION,
        "state_root": root.path(),
        "bind_host": "127.0.0.1",
        "bind_port": 0,
        "secret_fingerprint_salt": "0123456789abcdef",
        "instances": [{
            "alias": ALIAS, "instance_id": id.transport(),
            "application_id": "neutral.application", "adb_path": root.path().join("missing-adb.exe"),
            "connect": false, "host": "127.0.0.1", "port": 16384,
            "touch_backend": "adb_shell_input", "capture_backend": "adb"
        }]
    });
    serde_json::from_value::<ActingdConfigFile>(config).expect("typed config")
}

/// A `schedule.json` of one rule and one pool read from rows of a list, naming `pool_list_rows`.
fn schedule() -> Vec<u8> {
    let schedule = json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": [POOL_LIST_ROWS_SUBSYSTEM],
        "rules": [{
            "package_id": PACKAGE,
            "id": PACKAGE,
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
            "next_run_clamp_ms": 86_400_000,
            "yield_points": [],
            "expected_duration_ms": 60000,
            "cooldown_ms": 1000,
            "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
            "strategic_weight_milli": 1000
        }],
        "pools": [{
            "id": "pool.listed",
            "capacity": 1_000_000,
            "projection": {"kind": "none"},
            "observation": {
                "kind": "list_rows",
                "fact_key": "inventory.list.example_kind_a",
                "key_column": "id",
                "value_column": "count",
                "rows": [{"key": "item_a", "factor": 1}]
            },
            "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
            "group_delay": null
        }],
        "activity": [],
        "timeline": [],
        "held": []
    });
    serde_json::to_vec(&schedule).expect("schedule bytes")
}

#[test]
fn p1_this_runtime_names_no_pool_list_rows_and_refuses_a_file_that_requires_it() {
    let state = TempDir::new().expect("tempdir");
    let assembly = configuration(&state).assemble().expect("runtime assembly");
    let subsystems = assembly
        .manifest
        .subsystems
        .iter()
        .filter(|subsystem| subsystem.enabled)
        .map(|subsystem| subsystem.name.clone())
        .collect::<BTreeSet<_>>();
    assert!(
        subsystems.contains("standard_pack_schedule"),
        "{subsystems:?}"
    );
    assert!(
        !subsystems.contains(POOL_LIST_ROWS_SUBSYSTEM),
        "{subsystems:?}"
    );

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
    let root = read_standard_pack_root(
        StandardPackRootBytes {
            bundle: Some(bundle.as_slice()),
            ..StandardPackRootBytes::default()
        },
        GAME,
    )
    .unwrap_or_else(|issue| panic!("the root reads: {issue}"))
    .expect("the root holds bundle.json");
    let irreversible = IrreversiblePackages::new(BTreeMap::from([(
        PACKAGE.to_owned(),
        Some(DeclaredCapabilities {
            provides: BTreeMap::new(),
            remedies: BTreeSet::new(),
        }),
    )]));
    let schedule = schedule();
    let games = BTreeMap::from([(
        GAME.to_owned(),
        StandardPackGameFiles {
            schedule: &schedule,
            root: &root,
            maintenance: &[],
            irreversible: &irreversible,
        },
    )]);
    let instances = BTreeMap::from([(
        ALIAS.to_owned(),
        StandardPackBinding {
            game_id: GAME.to_owned(),
            server_id: SERVER.to_owned(),
        },
    )]);
    let failure = compose_standard_pack_rules(&StandardPackComposeInput {
        instances: &instances,
        games: &games,
        subsystems: Some(&subsystems),
        catalog_version: 1,
    })
    .expect_err("this Runtime refuses the file");
    let issue = &failure.issues()[0].issue;
    assert_eq!(issue.problem, StandardPackProblem::ScheduleRequiresMissing);
    assert_eq!(issue.pointer, "/requires/0");
}
