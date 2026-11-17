// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 KP-1 fix round 1 (register §1.448; `rd-1116/brief-463-kp1-fix1.md`, its third
// point; contracts/linear-steps.md, "Key steps" and "Tools"): `package check`, `package
// preflight`, `package bundle` and `standard-pack check`, `adopt` and `trial` read a pack
// directory through the source parser, which refused every step without a click before this fix
// round. Two of them, on a pack directory whose `linear_steps` task has a key step: `actinglab
// package check` admits it (exit 0, `admitted`, its `package_id`), and `actinglab package bundle`
// lays it out in a bundle (exit 0, `written`), its task copied with the key step as declared.
//
// The pack is a `linear_steps` source package of game `neutral`, server `test`, 2 x 1: `home` is a
// red first pixel, `list` a blue one and `done` a white one; a green second pixel is the key step's
// guard mark. `open` taps from `home` into `list`; `back` presses the back key from `list` to
// `done`.

use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

const PACKAGE_ID: &str = "neutral.kp1.back";
const TASK_FILE: &str = "resources/operations/task/task.json";

// The key step `back`, as the pack declares it.
fn key_step() -> Value {
    json!({"id": "back", "from": "list", "to": "done",
           "key": {"key": "back"},
           "guard": {"page_id": "list", "target_id": "second_green",
                     "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                     "color_probe": "second_green"},
           "expect_after": {"page_id": "done", "timeout_ms": 200}})
}

// Writes the pack as a source package directory at `directory`: the tap `open`, then `back`.
fn write_pack(directory: &Path) {
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": PACKAGE_ID,
        "execution_mode": "linear_steps",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 50,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    });
    let probe = |id: &str, x: u32, expected: [u8; 3]| {
        json!({"id": id,
               "region": {"mode": "rect", "rect": {"x": x, "y": 0, "width": 1, "height": 1}},
               "expected": expected})
    };
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "locale": "en-US",
        "goal": "KP-1 fixture",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [
            probe("first_red", 0, [255, 0, 0]),
            probe("first_blue", 0, [0, 0, 255]),
            probe("first_white", 0, [255, 255, 255]),
            probe("second_green", 1, [0, 255, 0])
        ],
        "page_rules": {
            "home": {"required": ["first_red"]},
            "list": {"required": ["first_blue"]},
            "done": {"required": ["first_white"]}
        },
        "entry_page": "home",
        "target_page": "done",
        "max_steps": 2,
        "scheduling_outcome": {"mappings": [
            {"outcome_key": "task_back", "effect": "no_designated_effect",
             "terminal_pages": ["done"]}
        ]},
        "operations": [
            {"id": "open", "from": "home", "to": "list",
             "click": {"kind": "point", "x": 1, "y": 0},
             "unguarded_trusted_coordinate": true,
             "expect_after": {"page_id": "list", "timeout_ms": 200}},
            key_step()
        ]
    });
    for (path, value) in [
        ("control.json", control),
        (
            "resources/operations/resources.json",
            json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
        ),
        (TASK_FILE, task),
    ] {
        let path = directory.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    }
}

#[test]
fn kp1_fix1_package_check_and_package_bundle_take_a_pack_with_a_key_step() {
    let temp = TempDir::new().unwrap();
    let packs_root = temp.path().join("packs");
    let pack = packs_root.join("kp1-back");
    write_pack(&pack);

    let checked = crate::run_cli(
        [
            "--json",
            "package",
            "check",
            "--package",
            pack.to_str().unwrap(),
        ],
        true,
    );
    assert_eq!(checked.exit_code(), 0, "{}", checked.envelope_json());
    let answer = checked.envelope.data.clone().unwrap();
    assert_eq!(answer["status"], "admitted", "{answer}");
    assert_eq!(answer["package_id"], PACKAGE_ID, "{answer}");

    let applications = temp.path().join("applications.json");
    fs::write(
        &applications,
        serde_json::to_vec_pretty(&json!({
            "schema_version": "actingcommand.applications.v1",
            "game": "neutral",
            "servers": {"test": {"application_id": "com.example.neutral", "label": "Test"}}
        }))
        .unwrap(),
    )
    .unwrap();
    let out = temp.path().join("bundle");
    let bundled = crate::run_cli(
        [
            "--json",
            "package",
            "bundle",
            "--applications",
            applications.to_str().unwrap(),
            "--packs-root",
            packs_root.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ],
        true,
    );
    assert_eq!(bundled.exit_code(), 0, "{}", bundled.envelope_json());
    let answer = bundled.envelope.data.clone().unwrap();
    assert_eq!(answer["status"], "written", "{answer}");
    let copied: Value = serde_json::from_slice(
        &fs::read(out.join("packs").join(PACKAGE_ID).join(TASK_FILE)).unwrap(),
    )
    .unwrap();
    assert_eq!(copied["operations"][1], key_step());
}
