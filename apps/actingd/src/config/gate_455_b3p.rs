// SPDX-License-Identifier: AGPL-3.0-only

// #455 B3-P (0.12.82; register §1.522; the coordinator's ruling 6 of 11-23 in its work order and
// fix round 1, not the owner's words; contracts/standard-pack-schedule.md, "`requires`";
// contracts/actingd-check-config.md, the row `pool_target_defaults`): this build's configuration
// manifest names `pool_target_defaults`, enabled, so the composer, given the subsystems the
// manifest names as the scheduler workbench and check-config give them, composes a
// `schedule.json` whose pool writes `target_defaults` and lists the subsystem in `requires`, and the
// composed catalog's pool holds the two numbers. The pattern of `gate_455_p1`; the names are made
// up.

use super::*;
use actingcommand_contract::{
    DeclaredCapabilities, IdentifierIssuer, IrreversiblePackages, StandardPackRootBytes,
    read_standard_pack_root,
};
use actingcommand_policy::{
    POOL_TARGET_DEFAULTS_SUBSYSTEM, PoolTargetDefaults, StandardPackBinding,
    StandardPackComposeInput, StandardPackGameFiles, compose_standard_pack_rules,
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

/// A `schedule.json` of one rule and one pool without a valuation that writes `target_defaults`,
/// naming `pool_target_defaults`.
fn schedule_file() -> Vec<u8> {
    let file = json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": [POOL_TARGET_DEFAULTS_SUBSYSTEM],
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
            "id": "pool.material",
            "capacity": 1_000_000,
            "projection": {"kind": "none"},
            "observation": {"kind": "fact", "fact_key": "resource.material"},
            "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
            "group_delay": null,
            "target_defaults": {"scale": 10, "importance_milli": 1000}
        }],
        "activity": [],
        "timeline": [],
        "held": []
    });
    serde_json::to_vec(&file).expect("schedule bytes")
}

#[test]
fn b3p_this_runtime_names_pool_target_defaults_and_composes_a_file_that_requires_it() {
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
        subsystems.contains(POOL_TARGET_DEFAULTS_SUBSYSTEM),
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
            variables: Some(Vec::new()),
        }),
    )]));
    let schedule_bytes = schedule_file();
    let games = BTreeMap::from([(
        GAME.to_owned(),
        StandardPackGameFiles {
            schedule: &schedule_bytes,
            root: &root,
            maintenance: &[],
            irreversible: &irreversible,
            catalog: None,
        },
    )]);
    let instances = BTreeMap::from([(
        ALIAS.to_owned(),
        StandardPackBinding {
            game_id: GAME.to_owned(),
            server_id: SERVER.to_owned(),
        },
    )]);
    let composed = compose_standard_pack_rules(&StandardPackComposeInput {
        instances: &instances,
        games: &games,
        subsystems: Some(&subsystems),
        catalog_version: 1,
    })
    .unwrap_or_else(|failure| panic!("this Runtime composes the file: {failure}"));
    let pools = &composed.compiled().catalog().pools.pools;
    assert!(
        pools.iter().any(|pool| pool.target_defaults
            == Some(PoolTargetDefaults {
                scale: 10,
                importance_milli: 1000
            })),
        "{pools:?}"
    );
}
