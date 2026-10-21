// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 slice E1 (train 0.12.9), the pack tooling says why it refuses; every refusal
// keeps refusing as before (a usage refusal, exit 2), only its code and typed values become
// specific, and no output names an absolute local path:
// - T2 (A40): an empty pack table is `package_bundle_packs_empty` in `package bundle` (an empty
//   `--packs-root`) and in `standard-pack check` (a `bundle.json` whose `packs` lists no pack),
//   with where it looked relative to the standard package;
// - T3 (A41): `standard-pack check` on a package whose `task.json` is no JSON document names that
//   file by its path inside the package, with the parser's position.
// The test for A39 (a zero-operation package names its rule) is in actinglab
// (`apps/actinglab/src/tests/gate_411_e1.rs`).
//
// A synthetic `neutral` game, server `test`, 16x9.

use crate::{
    PackageBundleRequest, StandardPackCheckRequest, check_standard_pack_root, package_bundle,
};
use actingcommand_contract::{LabError, LabErrorClass};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

const GAME: &str = "neutral";
const SERVER: &str = "test";
const PACKAGE_ID: &str = "neutral.test.task";
const PACKS_EMPTY: &str = "package_bundle_packs_empty";
const TASK_FILE: &str = "resources/operations/task/task.json";

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().expect("file parent")).expect("directory");
    fs::write(path, bytes).expect("file");
}

fn json_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec_pretty(value).expect("JSON")
}

// An installed game root `<temporary>/packages/neutral/` whose only root file is `bundle.json`,
// listing `packs` (each a package directory below the game root).
fn game_root(temp: &TempDir, packs: &[&str]) -> std::path::PathBuf {
    let root = temp.path().join("packages").join(GAME);
    fs::create_dir_all(&root).expect("game root");
    let listed = packs
        .iter()
        .map(|package_id| {
            json!({
                "package_id": package_id, "server": SERVER, "entry_task_id": "task",
                "path": format!("packs/{package_id}"), "file_count": 3, "byte_count": 64
            })
        })
        .collect::<Vec<_>>();
    write(
        &root.join("bundle.json"),
        &json_bytes(&json!({
            "schema_version": "actingcommand.bundle.v4", "game": GAME, "packs": listed
        })),
    );
    root
}

// The refusal's outcome (`details.outcome`).
fn outcome(error: &LabError) -> &Value {
    error
        .details
        .as_ref()
        .and_then(|details| details.get("outcome"))
        .unwrap_or_else(|| panic!("{}: no details.outcome: {}", error.code, error.message))
}

// The `path` values of an outcome and of its causes, in chain order.
fn paths(node: &Value, found: &mut Vec<String>) {
    if let Some(path) = node["values"]["path"].as_str() {
        found.push(path.to_owned());
    }
    for cause in node["causes"].as_array().into_iter().flatten() {
        paths(cause, found);
    }
}

// Requirement 3: the reported outcome names no local directory: `details` holds no part of the
// temporary directory's path, and every `path` value in it, its causes' included, is relative.
fn assert_outcome_paths_relative(case: &str, error: &LabError, temp: &TempDir) {
    let local = temp.path().display().to_string();
    let escaped = serde_json::to_string(&local).expect("escaped path");
    let details = serde_json::to_string(&error.details).expect("details JSON");
    assert!(
        !details.contains(&local) && !details.contains(escaped.trim_matches('"')),
        "{case}: the details name {local}: {details}"
    );
    let mut found = Vec::new();
    paths(outcome(error), &mut found);
    assert!(!found.is_empty(), "{case}: the outcome names no path");
    for path in found {
        assert!(
            !Path::new(&path).is_absolute(),
            "{case}: {path} is an absolute path"
        );
    }
}

// T2 (A40): with an empty pack table, `package bundle` and `standard-pack check` both refuse with
// the registered no-packs code, saying where they looked: `packs`, the standard package's pack
// directory, for `package bundle`; `bundle.json` at `/packs` for `standard-pack check`.
#[test]
fn e1_t2_an_empty_pack_table_is_refused_as_no_packs_in_bundle_and_standard_pack_check() {
    let bundle_inputs = TempDir::new().expect("temporary directory");
    let applications = bundle_inputs.path().join("applications.json");
    write(
        &applications,
        &json_bytes(&json!({
            "schema_version": "actingcommand.applications.v1",
            "game": GAME,
            "servers": {"test": {"application_id": "com.example.neutral", "label": "Test"}}
        })),
    );
    let packs_root = bundle_inputs.path().join("packs");
    fs::create_dir_all(&packs_root).expect("empty packs root");
    let bundled = package_bundle(PackageBundleRequest {
        applications,
        packs_root,
        out: bundle_inputs.path().join("bundle"),
        source: None,
        maintenance: None,
        root_files: Default::default(),
    });

    let installed = TempDir::new().expect("temporary directory");
    let root = game_root(&installed, &[]);
    let checked = check_standard_pack_root(StandardPackCheckRequest { root });

    let rows = [
        (
            "package bundle of an empty --packs-root",
            bundled.map(|_| ()),
            &bundle_inputs,
            "packs",
            None,
        ),
        (
            "standard-pack check of a bundle.json listing no pack",
            checked.map(|_| ()),
            &installed,
            "bundle.json",
            Some("/packs"),
        ),
    ];
    for (case, result, temp, path, pointer) in rows {
        let error = result.expect_err(case);
        assert_eq!(error.code, PACKS_EMPTY, "{case}: {}", error.message);
        assert_eq!(error.class, LabErrorClass::UsageValidation, "{case}");
        let outcome = outcome(&error);
        assert_eq!(outcome["code"], PACKS_EMPTY, "{case}: {outcome}");
        assert_eq!(outcome["category"], "error", "{case}: {outcome}");
        assert_eq!(outcome["values"]["path"], path, "{case}: {outcome}");
        match pointer {
            Some(pointer) => {
                assert_eq!(
                    outcome["values"]["json_pointer"], pointer,
                    "{case}: {outcome}"
                )
            }
            None => assert!(
                outcome["values"].get("json_pointer").is_none(),
                "{case}: {outcome}"
            ),
        }
        assert_outcome_paths_relative(case, &error, temp);
        let local = temp.path().display().to_string();
        assert!(
            !error.message.contains(&local),
            "{case}: the message names {local}: {}",
            error.message
        );
    }
}

// T3 (A41): `standard-pack check` of a root whose one package has a `task.json` cut off in its
// third line is refused `package_invalid` as before; its outcome names the package directory
// below the game root, caused by `source_operation_invalid` naming the file by its path inside
// the package with the parser's position (the line where the document breaks off).
#[test]
fn e1_t3_a_task_json_that_does_not_parse_is_named_by_its_path_inside_the_package() {
    let temp = TempDir::new().expect("temporary directory");
    let root = game_root(&temp, &[PACKAGE_ID]);
    let pack = root.join(PACKAGE_ID);
    write(
        &pack.join("control.json"),
        &json_bytes(&json!({
            "schema_version": "Lab-1y.control.v2",
            "package_id": PACKAGE_ID,
            "execution_mode": "navigable_route",
            "game": GAME,
            "server": SERVER,
            "resolution": {"width": 16, "height": 9},
            "entry_task_id": "task",
            "capture_interval_ms": 10,
            "step_timeout_ms": 600,
            "timeout_ms": 8000,
            "max_steps": 2
        })),
    );
    write(
        &pack.join("resources/operations/resources.json"),
        &json_bytes(&json!({"schema_version": "1.0", "resources": [], "resource_count": 0})),
    );
    // The task declaration breaks off inside its third line.
    write(
        &pack.join(TASK_FILE),
        b"{\n  \"schema_version\": \"0.9\",\n  \"task_id\": \"ta",
    );

    let error = check_standard_pack_root(StandardPackCheckRequest { root })
        .expect_err("a package whose task.json does not parse is admitted");
    assert_eq!(error.code, "package_invalid", "{}", error.message);
    assert_eq!(error.class, LabErrorClass::UsageValidation);
    let outcome = outcome(&error);
    assert_eq!(outcome["code"], "package_invalid", "{outcome}");
    assert_eq!(outcome["values"]["path"], PACKAGE_ID, "{outcome}");
    let cause = &outcome["causes"][0];
    assert_eq!(cause["relation"], "caused_by", "{outcome}");
    assert_eq!(cause["code"], "source_operation_invalid", "{outcome}");
    assert_eq!(cause["values"]["path"], TASK_FILE, "{outcome}");
    assert_eq!(cause["values"]["line"], 3, "{outcome}");
    assert!(
        cause["values"]["column"]
            .as_u64()
            .is_some_and(|column| column >= 1),
        "{outcome}"
    );
    assert_outcome_paths_relative("a task.json that does not parse", &error, &temp);
}
