// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #373 NAV-T (model M-373 v1.1 §3.9 second row, §11.2 row NT-3; wave-18 ruling 1; exec
// key 80): `actinglab standard-pack check --root <game root>` answers the check of an installed
// game root with the report of the packs whose entry page the page list does not hold, and
// refuses a package unfit for its part with the cross-check's code (exit 2), its outcome naming
// the package, its part and the problem.
//
// A synthetic `neutral` game root, server `test`, 16x9: the page list holds `home` (the first
// pixel blue) and `shop` (red); the feature package recognises both; the edge `home` to `shop`
// and the recovery chain's one step have their packages; one more pack starts on `stage`
// (green), which the list does not hold.

use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

const FEATURES: &str = "neutral.test.features";
const HOME_TO_SHOP: &str = "neutral.test.home_to_shop";
const RETURN_HOME: &str = "neutral.test.return_home";
const STAGE: &str = "neutral.test.stage";

// Writes a pack of the game as a source package directory: its execution mode, entry, target and
// one click per operation `(id, from, to)`, each page told by the first pixel's colour.
fn write_pack(
    directory: &Path,
    package_id: &str,
    mode: &str,
    (entry, target): (&str, &str),
    operations: &[(&str, &str, &str)],
) {
    let colour = |page: &str| match page {
        "home" => [0, 0, 255],
        "shop" => [255, 0, 0],
        "stage" => [0, 255, 0],
        _ => [255, 255, 0],
    };
    let mut pages = BTreeSet::from([entry, target]);
    for (_, from, to) in operations {
        pages.insert(*from);
        pages.insert(*to);
    }
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": package_id,
        "execution_mode": mode,
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 16, "height": 9},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    });
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "locale": "en-US",
        "goal": "NAV-T fixture",
        "coordinate_space": {"width": 16, "height": 9},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": pages
            .iter()
            .map(|page| json!({
                "id": format!("{page}_colour"),
                "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
                "expected": colour(page)
            }))
            .collect::<Vec<_>>(),
        "page_rules": pages
            .iter()
            .map(|page| ((*page).to_owned(), json!({"required": [format!("{page}_colour")]})))
            .collect::<serde_json::Map<_, _>>(),
        "entry_page": entry,
        "target_page": target,
        "operations": operations
            .iter()
            .map(|(id, from, to)| json!({
                "id": id,
                "from": from,
                "to": to,
                "click": {"kind": "point", "x": 1, "y": 0},
                "unguarded_trusted_coordinate": true,
                "retryable": false
            }))
            .collect::<Vec<_>>()
    });
    for (path, value) in [
        ("control.json", control),
        (
            "resources/operations/resources.json",
            json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
        ),
        ("resources/operations/task/task.json", task),
    ] {
        let path = directory.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    }
}

fn write_json(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

// The installed game root `<temporary>/packages/neutral/` whose edge `home` to `shop` names
// `edge_package`.
fn game_root(temp: &TempDir, edge_package: &str) -> std::path::PathBuf {
    let root = temp.path().join("packages").join("neutral");
    let packs: [(&str, &str, (&str, &str), &[(&str, &str, &str)]); 4] = [
        (
            FEATURES,
            "recognize_only",
            ("home", "home"),
            &[("see_shop", "shop", "home")],
        ),
        (
            HOME_TO_SHOP,
            "navigable_route",
            ("home", "shop"),
            &[("go", "home", "shop")],
        ),
        (
            RETURN_HOME,
            "navigable_route",
            ("shop", "home"),
            &[("back", "shop", "home")],
        ),
        (
            STAGE,
            "navigable_route",
            ("stage", "done"),
            &[("finish", "stage", "done")],
        ),
    ];
    for (package_id, mode, ends, operations) in packs {
        write_pack(&root.join(package_id), package_id, mode, ends, operations);
    }
    write_json(
        &root.join("bundle.json"),
        &json!({
            "schema_version": "actingcommand.bundle.v4",
            "game": "neutral",
            "packs": packs
                .iter()
                .map(|(package_id, ..)| json!({
                    "package_id": package_id,
                    "server": "test",
                    "entry_task_id": "task",
                    "path": format!("packs/{package_id}"),
                    "file_count": 3,
                    "byte_count": 64
                }))
                .collect::<Vec<_>>()
        }),
    );
    write_json(
        &root.join("pages.json"),
        &json!({
            "schema_version": "actingcommand.pages.v1",
            "game": "neutral",
            "features": [FEATURES],
            "pages": [{"id": "home", "kind": "page"}, {"id": "shop", "kind": "page"}]
        }),
    );
    write_json(
        &root.join("navigation.json"),
        &json!({
            "schema_version": "actingcommand.navigation.v1",
            "game": "neutral",
            "edges": [{"from": "home", "to": "shop", "package_id": edge_package}],
            "recovery_chain": [RETURN_HOME]
        }),
    );
    root
}

fn run_check(root: &Path) -> crate::CliResult {
    crate::run_cli(
        [
            "--json",
            "standard-pack",
            "check",
            "--root",
            root.to_str().unwrap(),
        ],
        true,
    )
}

// NT-3: the check of an installed game root answers the report: one
// `package_bundle_entry_page_unlisted` warning for the pack that starts on `stage`.
#[test]
fn nt_3_standard_pack_check_answers_the_report_of_an_installed_game_root() {
    let temp = TempDir::new().unwrap();
    let root = game_root(&temp, HOME_TO_SHOP);
    let result = run_check(&root);
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    let data = result.envelope.data.as_ref().expect("the check's answer");
    assert_eq!(data["status"], "checked");
    assert_eq!(data["game"], "neutral");
    assert_eq!(
        data["root_files"],
        json!(["bundle.json", "pages.json", "navigation.json"])
    );
    assert_eq!(data["pack_count"], 4);
    let warnings = data["warnings"].as_array().expect("the report");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0]["code"], "package_bundle_entry_page_unlisted");
    assert_eq!(warnings[0]["category"], "warning");
    assert_eq!(
        warnings[0]["values"],
        json!({"package_id": STAGE, "page_ids": ["stage"]})
    );
}

// NT-3: an edge whose package cannot be a layer (the `recognize_only` feature package) is
// refused with `package_bundle_navigation_package_invalid` (exit 2), its outcome naming the
// package, `navigation_role: edge` and `navigation_problem: role_ineligible`.
#[test]
fn nt_3_standard_pack_check_refuses_a_package_unfit_for_its_part() {
    let temp = TempDir::new().unwrap();
    let root = game_root(&temp, FEATURES);
    let result = run_check(&root);
    assert_eq!(result.exit_code(), 2, "{}", result.envelope_json());
    let error = result.envelope.error.as_ref().expect("a refusal");
    assert_eq!(error.code, "package_bundle_navigation_package_invalid");
    let outcome = &error.details.as_ref().expect("its outcome")["outcome"];
    assert_eq!(outcome["code"], "package_bundle_navigation_package_invalid");
    assert_eq!(
        outcome["values"],
        json!({
            "package_id": FEATURES,
            "navigation_role": "edge",
            "navigation_problem": "role_ineligible"
        })
    );
}
