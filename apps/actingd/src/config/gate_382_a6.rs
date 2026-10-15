// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #382 A-6 (test plan v2.2 §3.7 and §4 #13; inventory-apps X3): the start's
// configuration refusals, in process. actingd's start (`start_daemon`, apps/actingd/src/main.rs)
// loads the configuration and assembles it, the policy's checks and its registry modes included,
// before `RuntimeHost::start`; a refusal there is a start refusal before any record and any side
// effect (contracts/actingd-check-config.md:48-50 and :436; Workflow #361 model §2.C.2; Workflow
// #378 model §8.2 F8). Each row is one configuration with the code and the stage of its refusal.
// The stage is the step that refuses it: `load` reads and decodes the file, `assemble` checks the
// typed configuration (contracts/actingd-check-config.md:411-417; registered as `config_load` and
// `config_assemble`, Workflow #378 A6a). The process boundary of the same refusals stays with
// `invalid_startup_returns_nonzero` and `policy_configuration_is_refused_before_any_catalog_record`
// in apps/actingd/tests/process.rs.

use super::*;
use crate::codes::ActingdLocation;
use actingcommand_contract::IdentifierIssuer;
use actingcommand_policy::{HostResourceSnapshot, InstanceSnapshot, PoolValueSnapshot};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// The instance scope of the catalog-a example (contracts/scheduling/examples/catalog-a), the
/// catalog of every row: the alias of the one configured instance.
const INSTANCE_ALIAS: &str = "fixture-instance-a";
/// The time the configured pool value was observed; loading and assembling never evaluate it.
const OBSERVED_AT_UNIX_MS: u64 = 1_699_963_200_000;

/// The code and the stage of a start refusal; `None` when the start goes on.
type Refusal = Option<(&'static str, ActingdLocation)>;

/// Where actingd's start refuses the configuration at `path` before `RuntimeHost::start`, in the
/// order `start_daemon` runs: `load`, then `assemble`, whose policy part ends with the registry
/// modes (`PolicyAssembly::validate_registry_modes`).
fn start_refusal(path: &Path) -> Refusal {
    let file = match load(path) {
        Ok(file) => file,
        Err(code) => return Some((code.as_str(), ActingdLocation::ConfigLoad)),
    };
    file.assemble_or_refusal()
        .err()
        .map(|(code, _)| (code.as_str(), ActingdLocation::ConfigAssemble))
}

/// A policy configuration that starts, as the process tests write one
/// (`write_policy_execution_config`, apps/actingd/tests/process.rs): one fixture instance, the
/// catalog-a example with its approval, and its procedure bound for fixture simulation to a
/// package beside the file.
fn policy_configuration(root: &Path) -> Value {
    let policy_root = root.join("policy");
    fs::create_dir(&policy_root).expect("create the policy directory");
    let documents: [(&str, &[u8]); 4] = [
        (
            "tasks.json",
            include_bytes!("../../../../contracts/scheduling/examples/catalog-a/tasks.json"),
        ),
        (
            "pools.json",
            include_bytes!("../../../../contracts/scheduling/examples/catalog-a/pools.json"),
        ),
        (
            "activity.json",
            include_bytes!("../../../../contracts/scheduling/examples/catalog-a/activity.json"),
        ),
        (
            "timeline.json",
            include_bytes!("../../../../contracts/scheduling/examples/catalog-a/timeline.json"),
        ),
    ];
    for (name, bytes) in documents {
        fs::write(policy_root.join(name), bytes).expect("write a catalog document");
    }
    // A procedure package is located and named by its digest here, never read: its content is
    // admitted when the task runs (contracts/actingd-check-config.md:34-46).
    let package = b"neutral contained task package";
    fs::write(policy_root.join("task.zip"), package).expect("write the procedure package");
    let package_sha256 = format!("{:x}", Sha256::digest(package));
    let facts = EvaluationFacts {
        ledger_position: 0,
        fact_snapshot_id: "snapshot:actingd-config-a".to_owned(),
        facts: Vec::new(),
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: vec![InstanceSnapshot {
            instance_id: INSTANCE_ALIAS.to_owned(),
            server_id: "fixture-server-a".to_owned(),
            game_id: "fixture-game-a".to_owned(),
            host_id: "fixture-host-a".to_owned(),
            available: true,
            capability_operation_ids: vec!["operation.observe".to_owned()],
            preferred_task_ids: Vec::new(),
        }],
        priority_offsets: Vec::new(),
    };
    let resources = EvaluationResources {
        pools: vec![PoolValueSnapshot {
            pool_id: "fixture-pool-a".to_owned(),
            value: 10,
            observed_at_unix_ms: OBSERVED_AT_UNIX_MS,
        }],
        hosts: vec![HostResourceSnapshot {
            host_id: "fixture-host-a".to_owned(),
            cpu_available_milli: 1_000,
            gpu_available_milli: 1_000,
            io_available_milli: 1_000,
            host_responsiveness_basis_points: 10_000,
            third_party_pressure_basis_points: 0,
            heavy_dispatch_limit: 1,
            active_heavy_dispatches: 0,
        }],
    };
    let instance_id = IdentifierIssuer::new()
        .expect("issuer")
        .mint_instance_id()
        .expect("instance id");
    json!({
        "schema_version": CONFIG_SCHEMA_VERSION,
        "state_root": root.join("state"),
        "bind_host": "127.0.0.1",
        "bind_port": 0,
        "secret_fingerprint_salt": "0123456789abcdef",
        "policy": {
            "facts": facts,
            "resources": resources,
            "catalog": {
                "tasks": "policy/tasks.json",
                "pools": "policy/pools.json",
                "activity": "policy/activity.json",
                "timeline": "policy/timeline.json"
            },
            "catalog_approval_ids": ["approval:fixture-a"],
            "procedure_manifest": [{
                "procedure_ref": "procedure.observe",
                "package_digest": format!("sha256:{package_sha256}"),
                "operation_id": "operation.observe",
                "yield_points": ["after_observation"],
                "scheduled_execution": {
                    "mode": "fixture_simulation",
                    "package_path": "policy/task.zip"
                }
            }]
        },
        "instances": [{
            "alias": INSTANCE_ALIAS,
            "instance_id": instance_id.transport(),
            "fixture_backend": {
                "frames": [{"width": 2, "height": 1, "rgb": [0, 0, 255, 0, 255, 0]}],
                "max_inputs": 0
            }
        }]
    })
}

/// Test plan v2.2 §4 #13: the configurations the four retired process tests started are refused
/// by the start's load or assembly, each with its registered code
/// (contracts/outcome-codes/actingd.json, contract.json) at the stage the contract gives it
/// (contracts/actingd-check-config.md:411-417): an unknown scheduled execution mode does not
/// decode (`load`); a missing package path, approval ids that differ from the catalog's
/// `approval_refs` (:48-50) and a fixture-simulation binding on a physical instance (registry
/// modes are explicit per instance, apps gate key 2) are typed-configuration refusals
/// (`assemble`). The first row is the configuration every other row changes; it starts, so each
/// refusal is its row's change.
#[test]
fn gate_start_refusals_name_their_code_and_stage_before_the_runtime_host_starts() {
    type Change = fn(&mut Value);
    let rows: [(&str, Change, Refusal); 6] = [
        ("the unchanged configuration", |_| {}, None),
        (
            "fixture simulation bound to the procedure of a physical instance",
            |config| {
                let instance_id = config["instances"][0]["instance_id"].take();
                config["instances"][0] = json!({
                    "alias": INSTANCE_ALIAS,
                    "instance_id": instance_id,
                    "application_id": "neutral.application",
                    "adb_path": "must-not-run-adb",
                    "touch_backend": "maatouch",
                    "capture_backend": "adb",
                    "push_touch_tool": false
                });
            },
            Some((
                "scheduled_execution_backend_mode_mismatch",
                ActingdLocation::ConfigAssemble,
            )),
        ),
        (
            "fixture_simulation without a package path",
            |config| {
                config["policy"]["procedure_manifest"][0]["scheduled_execution"]
                    .as_object_mut()
                    .expect("scheduled execution object")
                    .remove("package_path");
            },
            Some((
                "procedure_package_path_missing",
                ActingdLocation::ConfigAssemble,
            )),
        ),
        (
            "device_registry without a package path",
            |config| {
                config["policy"]["procedure_manifest"][0]["scheduled_execution"]["mode"] =
                    json!("device_registry");
                config["policy"]["procedure_manifest"][0]["scheduled_execution"]
                    .as_object_mut()
                    .expect("scheduled execution object")
                    .remove("package_path");
            },
            Some((
                "procedure_package_path_missing",
                ActingdLocation::ConfigAssemble,
            )),
        ),
        (
            "an unknown scheduled execution mode",
            |config| {
                config["policy"]["procedure_manifest"][0]["scheduled_execution"]["mode"] =
                    json!("adb_backend");
            },
            Some(("config_decode_failed", ActingdLocation::ConfigLoad)),
        ),
        (
            "approval ids that do not match the catalog",
            |config| config["policy"]["catalog_approval_ids"] = json!(["approval:untrusted"]),
            Some((
                "policy_catalog_approval_mismatch",
                ActingdLocation::ConfigAssemble,
            )),
        ),
    ];
    for (case, change, refusal) in rows {
        let root = TempDir::new().expect("tempdir");
        let mut config = policy_configuration(root.path());
        change(&mut config);
        let path = root.path().join("actingd.json");
        fs::write(
            &path,
            serde_json::to_vec_pretty(&config).expect("configuration JSON"),
        )
        .expect("write the configuration");
        assert_eq!(start_refusal(&path), refusal, "{case}");
    }
}
