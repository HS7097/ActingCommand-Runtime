// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #477 CV-2 (0.12.80; register §1.508; coordinator work order `rd-1122/brief-477-cv2.md`,
// its ruling 4 and its test 4; contracts/standard-pack-schedule.md, "`requires`"): this build's
// configuration manifest names `catalog_business` among its enabled subsystems, and the composer,
// given the subsystems the manifest names as the scheduler workbench and check-config give them,
// composes a `schedule.json` that lists it with the root's version 2 `pack-catalog.json`: the
// composed task holds the business part the catalog gives its rule. The names are made up.

use super::*;
use actingcommand_contract::{
    DeclaredCapabilities, IdentifierIssuer, IrreversiblePackages, StandardPackRootBytes,
    read_standard_pack_root,
};
use actingcommand_policy::{
    CATALOG_BUSINESS_SUBSYSTEM, StandardPackBinding, StandardPackComposeInput,
    StandardPackGameFiles, compose_standard_pack_rules,
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use tempfile::TempDir;

const ALIAS: &str = "neutral.device";
const GAME: &str = "game-a";
const SERVER: &str = "server-a";
const PACKAGE: &str = "rule.plain";
const POOL: &str = "pool.plain";

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

/// A `schedule.json` of one rule written without a business member and one pool, naming
/// `catalog_business`.
fn schedule() -> Vec<u8> {
    let schedule = json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": [CATALOG_BUSINESS_SUBSYSTEM],
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
            "id": POOL,
            "capacity": 1000,
            "projection": {"kind": "none"},
            "observation": {"kind": "fact", "fact_key": "resource.plain"},
            "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
            "group_delay": null
        }],
        "activity": [],
        "timeline": [],
        "held": []
    });
    serde_json::to_vec(&schedule).expect("schedule bytes")
}

/// The root's version 2 catalog: the package produces 3 of [`POOL`] a run.
fn catalog() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema_version": "actingcommand.pack-catalog.v2",
        "game": GAME,
        "packs": [{
            "package_id": PACKAGE,
            "consumes": [],
            "produces": [{
                "pool_id": POOL,
                "direction": "produce",
                "amount": 3,
                "observation_source": "self_reported",
                "confidence_milli": 1000
            }]
        }]
    }))
    .expect("catalog bytes")
}

#[test]
fn cv2_this_runtime_names_catalog_business_and_composes_a_switched_root() {
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
        subsystems.contains(CATALOG_BUSINESS_SUBSYSTEM),
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
    let schedule = schedule();
    let catalog = catalog();
    let games = BTreeMap::from([(
        GAME.to_owned(),
        StandardPackGameFiles {
            schedule: &schedule,
            root: &root,
            maintenance: &[],
            irreversible: &irreversible,
            catalog: Some(&catalog),
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
    let tasks = &composed.compiled().catalog().tasks.tasks;
    let [task] = tasks.as_slice() else {
        panic!("one composed task: {tasks:?}");
    };
    assert_eq!(task.produces.len(), 1, "{task:?}");
    assert_eq!(task.produces[0].pool_id, POOL);
    assert_eq!(task.produces[0].units(), Some(3));
    assert!(task.consumes.is_empty(), "{task:?}");
}
