// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #392 / #401 DA-4 (model 392 v1.2 §4.1, §12.1 row DA-4; test row DA4-T4 of §13):
// `package bundle --display-layout <file>` lays the game's display declaration beside
// `bundle.json` as `display-layout.json`, byte for byte, and `actinglab standard-pack check
// --root` on the installed game root lists it among the game root's `root_files`.
//
// A synthetic `neutral` game, server `test`, 16x9, with one pack from `home` (the first pixel
// blue) to `shop` (red).

use super::*;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

const PACKAGE: &str = "neutral.test.home_to_shop";

/// The declaration as a person writes it: not canonical JSON, so a copy that is not byte for
/// byte would show.
const DECLARATION: &str = "{\n    \"schema_version\": \"actingcommand.display-layout.v1\",\n    \"game\": \"neutral\",\n    \"scale\": { \"by\": \"width\", \"area\": \"screen\" },\n    \"anchor_area\": \"screen\",\n    \"correction\": { \"radius_px\": 16 }\n}\n";

fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

/// The pack as a source package directory: its control, its resource table and its one task
/// from `home` to `shop`, each page told by the first pixel's colour.
fn write_pack(directory: &Path) {
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": PACKAGE,
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 16, "height": 9},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    });
    let probe = |page: &str, colour: [u8; 3]| {
        json!({
            "id": format!("{page}_colour"),
            "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
            "expected": colour
        })
    };
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "locale": "en-US",
        "goal": "DA-4 fixture",
        "coordinate_space": {"width": 16, "height": 9},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [probe("home", [0, 0, 255]), probe("shop", [255, 0, 0])],
        "page_rules": {
            "home": {"required": ["home_colour"]},
            "shop": {"required": ["shop_colour"]}
        },
        "entry_page": "home",
        "target_page": "shop",
        "operations": [{
            "id": "go",
            "from": "home",
            "to": "shop",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }]
    });
    for (path, value) in [
        ("control.json", control),
        (
            "resources/operations/resources.json",
            json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
        ),
        ("resources/operations/task/task.json", task),
    ] {
        write_json(&directory.join(path), &value);
    }
}

// DA4-T4: `package bundle --display-layout` writes `display-layout.json` beside `bundle.json`
// with the given file's bytes, whatever the given file is named; the installed game root made of
// that bundle (its `bundle.json`, its pack directory and its `display-layout.json`) passes
// `standard-pack check`, whose `root_files` list the declaration.
#[test]
fn da4_t4_package_bundle_lays_the_declaration_and_standard_pack_check_lists_it() {
    let temp = TempDir::new().unwrap();
    let sources = temp.path().join("sources");
    write_pack(&sources.join(PACKAGE));
    let applications = temp.path().join("applications.json");
    write_json(
        &applications,
        &json!({
            "schema_version": "actingcommand.applications.v1",
            "game": "neutral",
            "servers": {"test": {"application_id": "com.example.neutral", "label": "Test"}}
        }),
    );
    let declared = temp.path().join("declared").join("neutral-display.json");
    fs::create_dir_all(declared.parent().unwrap()).unwrap();
    fs::write(&declared, DECLARATION).unwrap();
    let out = temp.path().join("bundle");
    let bundled = run_cli(
        [
            "--json",
            "package",
            "bundle",
            "--applications",
            applications.to_str().unwrap(),
            "--packs-root",
            sources.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--display-layout",
            declared.to_str().unwrap(),
        ],
        true,
    );
    assert_eq!(bundled.exit_code(), 0, "{}", bundled.envelope_json());
    assert_eq!(
        bundled.envelope.data.as_ref().expect("the bundle's answer")["status"],
        "written"
    );
    assert_eq!(
        fs::read(out.join("display-layout.json")).unwrap(),
        DECLARATION.as_bytes(),
        "laid byte for byte"
    );

    let game_root = temp.path().join("packages").join("neutral");
    write_pack(&game_root.join(PACKAGE));
    for file in ["bundle.json", "display-layout.json"] {
        fs::copy(out.join(file), game_root.join(file)).unwrap();
    }
    let checked = run_cli(
        [
            "--json",
            "standard-pack",
            "check",
            "--root",
            game_root.to_str().unwrap(),
        ],
        true,
    );
    assert_eq!(checked.exit_code(), 0, "{}", checked.envelope_json());
    let data = checked.envelope.data.as_ref().expect("the check's answer");
    assert_eq!(data["status"], "checked");
    assert_eq!(
        data["root_files"],
        json!(["bundle.json", "display-layout.json"])
    );
    assert_eq!(data["pack_count"], 1);
}
