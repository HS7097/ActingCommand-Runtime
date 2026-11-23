// SPDX-License-Identifier: AGPL-3.0-only

// #477 CK-1 (0.12.83; register §1.510; the coordinator's ruling 4 of 11-23, no words of the
// owner's behind it; contracts/standard-pack-schedule.md, "`requires`"; contracts/actingd-check-
// config.md, `config_manifest`): this build's configuration manifest names `clock_monthly` among
// its enabled subsystems, and the composer, given the subsystems the manifest names as the
// scheduler workbench and check-config give them, composes a `schedule.json` that lists it and
// whose rule's clock is monthly: the composed task holds the monthly clock. The names are made up.

use super::*;
use actingcommand_contract::{
    DeclaredCapabilities, IdentifierIssuer, IrreversiblePackages, StandardPackRootBytes,
    read_standard_pack_root,
};
use actingcommand_policy::{
    CLOCK_MONTHLY_SUBSYSTEM, ClockSchedule, PredicateSpec, StandardPackBinding,
    StandardPackComposeInput, StandardPackGameFiles, compose_standard_pack_rules,
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use tempfile::TempDir;

const ALIAS: &str = "neutral.device";
const GAME: &str = "game-a";
const SERVER: &str = "server-a";
const PACKAGE: &str = "rule.monthly";

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

/// A `schedule.json` of one rule whose trigger is a monthly clock (day 1 at 04:00, nine hours
/// ahead of UTC), naming `clock_monthly`.
fn schedule() -> Vec<u8> {
    let document = json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": [CLOCK_MONTHLY_SUBSYSTEM],
        "rules": [{
            "package_id": PACKAGE,
            "id": PACKAGE,
            "priority": 100,
            "trigger": {
                "kind": "clock",
                "schedule": {
                    "kind": "monthly",
                    "clock_source": {
                        "kind": "server",
                        "timezone_id": "fixed/plus-nine",
                        "utc_offset_minutes": 540,
                        "dst_offset_minutes": 0,
                        "maintenance_drift_ms": 0
                    },
                    "day_of_month": 1,
                    "minute_of_day": 240
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
        "pools": [],
        "activity": [],
        "timeline": [],
        "held": []
    });
    serde_json::to_vec(&document).expect("schedule bytes")
}

#[test]
fn ck1_this_runtime_names_clock_monthly_and_composes_a_file_that_requires_it() {
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
        subsystems.contains(CLOCK_MONTHLY_SUBSYSTEM),
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
    let file = schedule();
    let games = BTreeMap::from([(
        GAME.to_owned(),
        StandardPackGameFiles {
            schedule: &file,
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
    let tasks = &composed.compiled().catalog().tasks.tasks;
    let [task] = tasks.as_slice() else {
        panic!("one composed task: {tasks:?}");
    };
    assert!(
        matches!(
            &task.trigger,
            PredicateSpec::Clock {
                schedule: ClockSchedule::Monthly {
                    day_of_month: 1,
                    minute_of_day: 240,
                    ..
                },
                ..
            }
        ),
        "{task:?}"
    );
}
