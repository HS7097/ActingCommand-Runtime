// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 slice E4 (register §1.156), the two items `actinglab package check` answers:
// - T2 (B28; E4-2; exec key 131): a page rule whose `any_of` has the wrong shape is refused as
//   before (`package_invalid`, exit 2, `details.code` `resource_declaration_invalid` at the
//   refused member), and the refusal's outcome says the shape `any_of` must have as a typed
//   value: `expected_shape` `array_of_arrays`.
// - T3 (B34; E4-3; apps key 88): a click whose declared box is one point (a `point` click, a box
//   of 1×1) is a warning that names the operation and the box; the package still passes. A box
//   larger than one point is not warned on.
// T1 (B27) is `actingctl`'s `mcp::tests::gate_411_e4`; T4 (B39 A part) is `runtime-host`'s
// `tests::gate_373_nav5::gate_411_e4`.

use crate::package_tool::gate_390_tool1::neutral_package;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

const TASK_FILE: &str = "resources/operations/task/task.json";
const DECLARATION_INVALID: &str = "resource_declaration_invalid";
const SINGLE_POINT: &str = "package_check_click_box_single_point";
/// The one operation of the fixture package.
const OPERATION: &str = "open_terminal";

/// The fixture package `package check` admits, at `root`, with its task document changed by
/// `change`.
fn package_with(root: &Path, change: impl FnOnce(&mut Value)) {
    let mut task = neutral_package(root);
    change(&mut task);
    fs::write(
        root.join(TASK_FILE),
        format!("{}\n", serde_json::to_string_pretty(&task).unwrap()),
    )
    .unwrap();
}

fn check(package: &Path) -> crate::CliResult {
    crate::run_cli(
        [
            "--json",
            "package",
            "check",
            "--package",
            package.to_str().unwrap(),
        ],
        true,
    )
}

#[test]
fn e4_t2_an_any_of_of_the_wrong_shape_is_refused_naming_the_shape_it_must_have() {
    // `any_of` is a list of groups, each a list of target ids: a list of ids, or one id.
    for (label, any_of, field_path) in [
        (
            "a list of ids",
            json!(["home_red"]),
            "/page_rules/home/any_of/0",
        ),
        ("one id", json!("home_red"), "/page_rules/home/any_of"),
    ] {
        let temp = TempDir::new().unwrap();
        let package = temp.path().join("neutral.any_of.source");
        package_with(&package, |task| {
            task["page_rules"]["home"]["any_of"] = any_of;
        });

        let result = check(&package);

        let envelope = result.envelope_json();
        assert_eq!(result.exit_code(), 2, "{label}: {envelope}");
        let error = result
            .envelope
            .error
            .as_ref()
            .unwrap_or_else(|| panic!("{label}: not refused: {envelope}"));
        assert_eq!(error.code, "package_invalid", "{label}: {envelope}");
        let details = error
            .details
            .as_ref()
            .unwrap_or_else(|| panic!("{label}: a refusal without details: {envelope}"));
        assert_eq!(details["code"], DECLARATION_INVALID, "{label}: {envelope}");
        assert_eq!(
            details["declaration"]["field_path"], field_path,
            "{label}: {envelope}"
        );
        assert_eq!(
            details["declaration"]["reason"], "invalid_type",
            "{label}: {envelope}"
        );
        let outcome = &details["outcome"];
        assert_eq!(outcome["code"], DECLARATION_INVALID, "{label}: {envelope}");
        // B28: the shape `any_of` must have, as a typed value.
        assert_eq!(
            outcome["values"]["expected_shape"], "array_of_arrays",
            "{label}: {envelope}"
        );
    }
}

#[test]
fn e4_t3_a_click_box_of_one_point_is_a_warning_and_the_package_still_passes() {
    // The fixture's click is a point at (1, 0) in its 2×1 coordinate space.
    for (label, click, warned) in [
        ("a point click", None, Some([1, 0, 1, 1])),
        (
            "a box of 1×1",
            Some(json!({"kind": "rect", "x": 1, "y": 0, "width": 1, "height": 1})),
            Some([1, 0, 1, 1]),
        ),
        (
            "a box of 2×1",
            Some(json!({"kind": "rect", "x": 0, "y": 0, "width": 2, "height": 1})),
            None,
        ),
    ] {
        let temp = TempDir::new().unwrap();
        let package = temp.path().join("neutral.click.source");
        package_with(&package, |task| {
            if let Some(click) = click {
                task["operations"][0]["click"] = click;
            }
        });

        let result = check(&package);

        let envelope = result.envelope_json();
        // The package still passes.
        assert_eq!(result.exit_code(), 0, "{label}: {envelope}");
        let data = result
            .envelope
            .data
            .as_ref()
            .unwrap_or_else(|| panic!("{label}: no answer: {envelope}"));
        assert_eq!(data["status"], "admitted", "{label}: {envelope}");
        let warnings = data["warnings"]
            .as_array()
            .unwrap_or_else(|| panic!("{label}: no warnings list: {envelope}"));
        match warned {
            Some([x, y, width, height]) => {
                let [warning] = warnings.as_slice() else {
                    panic!("{label}: not one warning: {envelope}");
                };
                assert_eq!(warning["code"], SINGLE_POINT, "{label}: {envelope}");
                assert_eq!(warning["category"], "warning", "{label}: {envelope}");
                assert_eq!(
                    warning["values"],
                    json!({
                        "pack_operation_id": OPERATION,
                        "box_x": x,
                        "box_y": y,
                        "box_width": width,
                        "box_height": height,
                    }),
                    "{label}: {envelope}"
                );
            }
            None => assert!(warnings.is_empty(), "{label}: {envelope}"),
        }
    }
}
