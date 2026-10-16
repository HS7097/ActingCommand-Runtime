// SPDX-License-Identifier: AGPL-3.0-only

// Test plan v2.2 E-4 G4b (exec key 12), the offline half: `package bundle --maintenance`
// (bundle v3 then, bundle v4 since Workflow #336 PKG-S5) judges the shared fixture set EXEC-I6 (`tests/support/exec_i6.rs`) with the
// Runtime's own predicates: `PreparedContainedTask::describe_path` performs the Runtime's task
// preparation, and every complete chain goes through the shared `PrerequisiteChain`, so a chain
// the Runtime refuses is refused here under the same code (`contracts/package-reference.md`,
// "Maintenance declarations"). The Runtime half is runtime-host's
// `tests::gate_e4`. Requirement basis: rd-1009/tests/req-basis-el.md section E-4, row "G4b".

#[allow(dead_code)]
#[path = "../../../tests/support/exec_i6.rs"]
mod exec_i6;

use crate::{PackageBundleRequest, package_bundle};
use actingcommand_contract::LabResult;
use exec_i6::{GAME, I6Case, ROOT, SERVER};
use serde_json::json;
use std::fs;
use tempfile::TempDir;

const MAINTENANCE_INVALID: &str = "package_bundle_maintenance_invalid";
// How a kernel refusal reads inside the bundle's message (`ContainedTaskError`'s display).
const KERNEL_PREFIX: &str = "contained task error ";

// Bundles the root package of `case` and the packages of `case.bound`, with an empty maintenance
// declaration (the full qualification). Workflow #373 NAV-3a (M-373 §3.8, §4.3): the bundle is its
// game's registry, so the packages it holds are the bound ones, as runtime-host's `gate_e4` lists
// them in the game root's `bundle.json`; the others are not in the bundle.
fn bundle(case: &I6Case) -> (TempDir, LabResult<crate::PackageBundleResponse>) {
    let temp = TempDir::new().expect("temporary directory");
    let applications = temp.path().join("applications.json");
    fs::write(
        &applications,
        serde_json::to_vec_pretty(&json!({
            "schema_version": "actingcommand.applications.v1",
            "game": GAME,
            "servers": {SERVER: {
                "application_id": "com.example.neutral",
                "label": "Test",
                "default_package_id": ROOT
            }}
        }))
        .expect("applications JSON"),
    )
    .expect("applications table");
    let packs_root = temp.path().join("packs");
    for pack in &case.packs {
        if pack.package_id == ROOT || case.bound.contains(&pack.package_id) {
            exec_i6::write_pack(&packs_root.join(pack.package_id), pack);
        }
    }
    let result = package_bundle(PackageBundleRequest {
        applications,
        packs_root,
        out: temp.path().join("bundle"),
        source: None,
        maintenance: Some(Vec::new()),
        root_files: Default::default(),
    });
    (temp, result)
}

// The kernel code a bundle refusal names in its message.
fn kernel_code(message: &str) -> Option<&str> {
    let (_, rest) = message.split_once(KERNEL_PREFIX)?;
    rest.split(|character: char| !(character.is_ascii_lowercase() || character == '_'))
        .next()
}

// package-reference.md: the bundle check performs the Runtime's task preparation and checks
// every complete chain against the bundle's own packs. The set whose root has one bound
// prerequisite package is written (bundle v4 with its maintenance declarations).
#[test]
fn gate_e4_bundle_v3_admits_the_fixture_set_the_runtime_admits() {
    let (_temp, result) = bundle(&exec_i6::admitted_case());
    let response = result.unwrap_or_else(|error| panic!("{}: {}", error.code, error.message));
    assert_eq!(response.status, "written");
}

// package-reference.md: "The shared `PrerequisiteChain` predicate owns game/server/resolution
// compatibility, cycle detection, three declared layers"; Host consumes "this same predicate".
// Each refused set of EXEC-I6 is refused as a maintenance declaration naming the kernel code the
// Runtime refuses its run with (runtime-host `tests::gate_e4`).
#[test]
fn gate_e4_bundle_v3_refuses_each_fixture_chain_with_the_runtimes_code() {
    for case in exec_i6::refused_cases() {
        let (_temp, result) = bundle(&case);
        let error = match result {
            Ok(response) => panic!(
                "{}: the bundle was written: {:?}",
                case.name, response.index
            ),
            Err(error) => error,
        };
        assert_eq!(
            error.code, MAINTENANCE_INVALID,
            "{}: {}",
            case.name, error.message
        );
        assert_eq!(
            kernel_code(&error.message),
            Some(case.code),
            "{}: {}",
            case.name,
            error.message
        );
    }
}
