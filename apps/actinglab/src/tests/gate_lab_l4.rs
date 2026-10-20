// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 Lab L4 (v0.12.x; register §1.120; exec gate key 98): the offline capability
// listing names the backends the Runtime can use. `capabilities` lists `scrcpy` (a handset only),
// keeps `auto` and `auto-fastest` as refused by actingd (`capture_backend_must_be_explicit`)
// instead of declared support, and lists the four touch backends with the gestures each
// performs, as declared support, unverified; `schema control` and `session instance registry`
// name the same explicit choices (contracts/actinglab-capabilities.md). Requirement basis:
// rd-1019/plan-lab-backends/plan.md §4 L4 R2 and §1.2.

use super::*;
use std::collections::{BTreeMap, BTreeSet};

const VERTICAL_BRAKE: &str = "single_touch_drag_with_vertical_brake_v1";
const PAUSE_AND_BRAKE: &str = "single_touch_drag_with_pause_and_brake_v1";
// Workflow #406 T3/T4: the path drag.
const PATH_DRAG: &str = "single_touch_path_drag_v1";

fn strings(value: &Value) -> BTreeSet<String> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("expected a list: {value}"))
        .iter()
        .map(|item| {
            item.as_str()
                .unwrap_or_else(|| panic!("expected a string: {item}"))
                .to_string()
        })
        .collect()
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|item| item.to_string()).collect()
}

fn by_id(list: &Value) -> BTreeMap<String, Value> {
    list.as_array()
        .unwrap_or_else(|| panic!("expected a list: {list}"))
        .iter()
        .map(|entry| (entry["id"].as_str().unwrap().to_string(), entry.clone()))
        .collect()
}

fn offline_capabilities() -> Value {
    let temp = TempDir::new().unwrap();
    let _runtime_env = use_runtime_state_root(&temp.path().join("runtime-must-not-exist"));
    set_missing_config_env();
    let result = run_cli(["--json", "capabilities"], true);
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    result.envelope.data.clone().unwrap()
}

fn assert_declared_unverified(entry: &Value) {
    assert_eq!(entry["declared_support"], true, "{entry}");
    assert_eq!(entry["status"], "unverified", "{entry}");
    assert_eq!(entry["available"], false, "{entry}");
    assert_eq!(entry["availability_checked"], false, "{entry}");
    assert_eq!(entry["execution_authority"], "runtime", "{entry}");
}

#[test]
fn l4_capabilities_list_scrcpy_and_refuse_the_automatic_capture_choices() {
    let _guard = env_lock();
    let data = offline_capabilities();
    let capture = by_id(&data["capture_backends"]);

    assert_eq!(
        capture.keys().cloned().collect::<BTreeSet<_>>(),
        set(&[
            "adb",
            "droidcast_raw",
            "nemu_ipc",
            "scrcpy",
            "auto",
            "auto-fastest"
        ])
    );
    for id in ["adb", "droidcast_raw", "nemu_ipc", "scrcpy"] {
        assert_declared_unverified(&capture[id]);
    }
    assert_eq!(capture["scrcpy"]["device_kinds"], json!(["handset"]));
    for id in ["auto", "auto-fastest"] {
        let entry = &capture[id];
        assert_eq!(entry["declared_support"], false, "{entry}");
        assert_eq!(entry["status"], "unavailable", "{entry}");
        assert_eq!(entry["available"], false, "{entry}");
        assert_eq!(
            entry["reason_code"], "capture_backend_must_be_explicit",
            "{entry}"
        );
    }
}

#[test]
fn l4_capabilities_list_the_four_touch_backends_with_the_gestures_each_performs() {
    let _guard = env_lock();
    let data = offline_capabilities();
    let touch = by_id(&data["touch_backends"]);

    // Workflow #406 T4 (adapted): since #406 T3 every segmented backend performs the path drag,
    // and `adb_shell_input` performs the segmented plans through `input motionevent` (plan Q18).
    let expected = [
        (
            "nemu_ipc",
            set(&["tap", VERTICAL_BRAKE, PAUSE_AND_BRAKE, PATH_DRAG]),
        ),
        (
            "maatouch",
            set(&[
                "tap",
                "long_tap",
                "swipe",
                VERTICAL_BRAKE,
                PAUSE_AND_BRAKE,
                PATH_DRAG,
                "key",
                "text",
            ]),
        ),
        (
            "minitouch",
            set(&[
                "tap",
                "long_tap",
                "swipe",
                VERTICAL_BRAKE,
                PAUSE_AND_BRAKE,
                PATH_DRAG,
            ]),
        ),
        (
            "adb_shell_input",
            set(&[
                "tap",
                "long_tap",
                "swipe",
                VERTICAL_BRAKE,
                PAUSE_AND_BRAKE,
                PATH_DRAG,
            ]),
        ),
    ];
    assert_eq!(
        touch.keys().cloned().collect::<BTreeSet<_>>(),
        expected
            .iter()
            .map(|(id, _)| id.to_string())
            .collect::<BTreeSet<_>>()
    );
    for (id, gestures) in expected {
        let entry = &touch[id];
        assert_eq!(strings(&entry["gestures"]), gestures, "{entry}");
        assert_declared_unverified(entry);
    }
}

#[test]
fn l4_schema_control_and_the_instance_registry_name_the_explicit_runtime_choices() {
    let _guard = env_lock();
    set_missing_config_env();
    let schema = run_cli(["--json", "schema", "control"], true);
    let registry = run_cli(["--json", "session", "instance", "registry"], true);

    let capture = set(&["adb", "droidcast_raw", "nemu_ipc", "scrcpy"]);
    let touch = set(&["nemu_ipc", "maatouch", "minitouch", "adb_shell_input"]);
    assert_eq!(schema.exit_code(), 0, "{}", schema.envelope_json());
    let schema = schema.envelope.data.as_ref().unwrap();
    assert_eq!(strings(&schema["capture_backend"]), capture, "{schema}");
    assert_eq!(strings(&schema["touch_backend"]), touch, "{schema}");
    assert_eq!(registry.exit_code(), 0, "{}", registry.envelope_json());
    let registry = registry.envelope.data.as_ref().unwrap();
    assert_eq!(
        strings(&registry["capture_backends"]),
        capture,
        "{registry}"
    );
    assert_eq!(strings(&registry["touch_backends"]), touch, "{registry}");
}
