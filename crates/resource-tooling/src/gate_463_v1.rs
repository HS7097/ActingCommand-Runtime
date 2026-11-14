// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 V1 (register §1.378; contracts/package-variables.md, "The root check"; coordinator
// work order `rd-1114/brief-463-v1-package-variables.md`): `actinglab standard-pack check` reads
// the variables every package of an installed game root declares, with the Runtime's metadata
// reader, and runs the contract crate's root check on them:
// - `v1_check_reports_each_problem_at_the_package_and_lists_what_applies`: a variable two packages
//   declare byte-identically applies once; one whose declarations differ is `variable_conflict` at
//   each declaration, and one whose name a `goals.json` heading uses is `variable_conflict` at its
//   `name`, each one `standard_pack_root_invalid` warning naming the declaring package's
//   `control.json`; the root is checked, the placement of the shared variable passes, and
//   `variable_provenance` lists only the variable that applies, by its default's origin;
// - `v1_a_root_without_variables_answers_without_the_listing`: the same root whose packages
//   declare none answers no `variable_provenance` (the answer as before), and one whose package
//   declares a variable written by the model lists it under `model`.
// A made-up game `neutral`, server `test`, 16x9; every package, variable and text is a
// placeholder.

use crate::{StandardPackCheckRequest, StandardPackCheckResponse, check_standard_pack_root};
use actingcommand_contract::outcome::Outcome;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const GAME: &str = "neutral";
const SERVER: &str = "test";
const ROOT_INVALID: &str = "standard_pack_root_invalid";

fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
    fs::write(path, serde_json::to_vec_pretty(value).expect("JSON")).expect("a file");
}

/// Writes the package `package_id` as a package directory: its control (with `variables` when
/// given), its resource table and its one task, from page `home` (a blue first pixel) to page
/// `page_b` (a red one).
fn write_pack(directory: &Path, package_id: &str, variables: Option<&Value>) {
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": "navigable_route",
        "game": GAME,
        "server": SERVER,
        "resolution": {"width": 16, "height": 9},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    });
    if let Some(variables) = variables {
        control["variables"] = variables.clone();
    }
    let probe = |page: &str, expected: [u8; 3]| {
        json!({
            "id": format!("{page}_colour"),
            "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
            "expected": expected
        })
    };
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": GAME,
        "server_scope": [SERVER],
        "locale": "en-US",
        "goal": "V1 fixture",
        "coordinate_space": {"width": 16, "height": 9},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [probe("home", [0, 0, 255]), probe("page_b", [255, 0, 0])],
        "page_rules": {
            "home": {"required": ["home_colour"]},
            "page_b": {"required": ["page_b_colour"]}
        },
        "entry_page": "home",
        "target_page": "page_b",
        "operations": [{
            "id": "go", "from": "home", "to": "page_b",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true, "retryable": false
        }]
    });
    write_json(&directory.join("control.json"), &control);
    write_json(
        &directory.join("resources/operations/resources.json"),
        &json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
    );
    write_json(
        &directory.join("resources/operations/task/task.json"),
        &task,
    );
}

/// An installed game root `<temporary>/packages/neutral/` whose `bundle.json` lists `packs`, each
/// with the variables it declares, in this order; no other root file.
fn installed(packs: &[(&str, Option<Value>)]) -> (TempDir, PathBuf) {
    let temp = TempDir::new().expect("temporary directory");
    let game_root = temp.path().join("packages").join(GAME);
    let mut listed = Vec::new();
    for (package_id, variables) in packs {
        write_pack(&game_root.join(package_id), package_id, variables.as_ref());
        listed.push(json!({
            "package_id": package_id, "server": SERVER, "entry_task_id": "task",
            "path": format!("packs/{package_id}"), "file_count": 3, "byte_count": 64
        }));
    }
    write_json(
        &game_root.join("bundle.json"),
        &json!({"schema_version": "actingcommand.bundle.v4", "game": GAME, "packs": listed}),
    );
    (temp, game_root)
}

fn check(game_root: &Path) -> StandardPackCheckResponse {
    check_standard_pack_root(StandardPackCheckRequest {
        root: game_root.to_owned(),
    })
    .unwrap_or_else(|error| panic!("the root is checked: {}: {}", error.code, error.message))
}

/// The `standard_pack_root_invalid` warnings of an answer, each its values.
fn root_invalid_values(warnings: &[Outcome]) -> Vec<Value> {
    warnings
        .iter()
        .filter(|warning| warning.code().as_str() == ROOT_INVALID)
        .map(|warning| json!(warning.values()))
        .collect()
}

/// The values of the warning of `problem` at `pointer` in the control declaration of the package
/// `package_id` of `game_root`.
fn variable_warning(game_root: &Path, package_id: &str, problem: &str, pointer: &str) -> Value {
    let path = std::path::absolute(game_root)
        .expect("absolute game root")
        .join(package_id)
        .join("control.json");
    json!({
        "game": GAME,
        "path": path.to_string_lossy(),
        "standard_pack_problem": problem,
        "json_pointer": pointer
    })
}

/// The variable `floor`, a number of 0 to 200 on a step of 10.
fn floor() -> Value {
    json!({"name": "floor", "type": "integer", "title": {"en": "Keep at least"},
           "min": 0, "max": 200, "step": 10, "default": 0})
}

/// A switch named `name` with its `default`.
fn switch(name: &str, default: bool) -> Value {
    json!({"name": name, "type": "switch", "title": {"en": "Switch"}, "default": default})
}

/// The choice `pick`, its default written by the model.
fn pick() -> Value {
    json!({"name": "pick", "type": "choice", "title": {"en": "Item"},
           "options": [{"value": "item_a", "title": {"en": "Item A"}},
                       {"value": "item_b", "title": {"en": "Item B"}}],
           "default": "item_b", "default_origin": "model"})
}

#[test]
fn v1_check_reports_each_problem_at_the_package_and_lists_what_applies() {
    let (_temp, game_root) = installed(&[
        ("neutral.a", Some(json!([floor(), pick()]))),
        ("neutral.b", Some(json!([floor(), switch("extra", false)]))),
        ("neutral.c", Some(json!([switch("extra", true)]))),
    ]);
    write_json(
        &game_root.join("goals.json"),
        &json!({
            "schema_version": "actingcommand.goals.v1",
            "game": GAME,
            "entries": [
                {"name": "pick", "type": "heading", "title": {"en": "Picks"}},
                {"name": "floor", "type": "variable", "block": "number"}
            ]
        }),
    );
    let checked = check(&game_root);
    assert_eq!(checked.status, "checked");
    assert_eq!(checked.root_files, ["bundle.json", "goals.json"]);
    assert_eq!(
        root_invalid_values(&checked.warnings),
        [
            variable_warning(
                &game_root,
                "neutral.a",
                "variable_conflict",
                "/variables/1/name"
            ),
            variable_warning(&game_root, "neutral.b", "variable_conflict", "/variables/1"),
            variable_warning(&game_root, "neutral.c", "variable_conflict", "/variables/0"),
        ]
    );
    assert_eq!(
        serde_json::to_value(&checked.variable_provenance).expect("the listing"),
        json!({"pack": ["floor"], "model": []})
    );
}

#[test]
fn v1_a_root_without_variables_answers_without_the_listing() {
    let (_plain, plain_root) = installed(&[("neutral.a", None), ("neutral.b", None)]);
    let plain = serde_json::to_value(check(&plain_root)).expect("the answer");
    assert!(plain.get("variable_provenance").is_none(), "{plain}");

    let (_temp, game_root) =
        installed(&[("neutral.a", None), ("neutral.b", Some(json!([pick()])))]);
    let declaring = serde_json::to_value(check(&game_root)).expect("the answer");
    assert_eq!(
        declaring["variable_provenance"],
        json!({"pack": [], "model": ["pick"]}),
        "{declaring}"
    );
    assert!(
        root_invalid_values(&check(&game_root).warnings).is_empty(),
        "{declaring}"
    );
}
