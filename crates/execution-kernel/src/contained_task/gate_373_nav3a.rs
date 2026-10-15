// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #373 NAV-3a (model M-373 v1.1 §4.2; test row N3a-1, exec key 67): a
// `navigable_route` package's control may declare `prerequisite_package_id`, as a `linear_steps`
// package's may; `in_page_guard` and `recognize_only` still may not. The NAV-3a follow-up (exec
// key 71) adds the detail reason `prerequisite_requires_page_entry`.

use super::*;
use serde_json::json;

// A control of `mode` that declares the prerequisite package `neutral.test.lead`.
fn declaring(mode: &str) -> TaskControl {
    serde_json::from_value(json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.test.main",
        "execution_mode": mode,
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 16, "height": 9},
        "entry_task_id": "task",
        "prerequisite_package_id": "neutral.test.lead"
    }))
    .expect("task control")
}

// N3a-1: the declaration is admitted for a `navigable_route` package (and still for a
// `linear_steps` one); it is refused for the two modes this slice does not open.
#[test]
fn n3a_1_a_navigable_route_package_may_declare_a_prerequisite_package() {
    for mode in ["navigable_route", "linear_steps"] {
        if let Err(error) = declaring(mode).validate() {
            panic!("{mode}: {error}");
        }
    }
    for mode in ["in_page_guard", "recognize_only"] {
        let error = declaring(mode)
            .validate()
            .expect_err("the declaration is refused");
        assert_eq!(
            error.code().as_str(),
            ExecutionKernelCode::ContainedTaskControlInvalid.as_str(),
            "{mode}"
        );
    }
}

// The pages `home` and `shop` of the neutral game.
fn detector() -> PageDetector {
    PageDetector::new(
        serde_json::from_value(json!({
            "schema_version": "0.6",
            "pages": [
                {"id": "neutral/home", "required": ["page/home"]},
                {"id": "neutral/shop", "required": ["page/shop"]}
            ]
        }))
        .expect("page set"),
    )
    .expect("page detector")
}

// A page-graph task to `home` whose `entry_page` is `entry`, or that has none.
fn program(entry: Option<&str>) -> TaskProgram {
    let mut program = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 16, "height": 9},
        "target_page": "home",
        "operations": []
    });
    if let Some(entry) = entry {
        program["entry_page"] = json!(entry);
    }
    serde_json::from_value(program).expect("task program")
}

// Workflow #373 NAV-3a (wave 14 ruling NAV-3a item 3; contracts/linear-steps.md, "Prerequisite
// packages"; register §1.67 N3A-5; exec key 71): a `navigable_route` package that declares a
// prerequisite package needs an entry page for it to lead to. Without one (no `entry_page`, or
// `any`) its admission is refused with `contained_task_control_invalid`,
// `reason=prerequisite_requires_page_entry`; the same task declaring none is admitted with no
// entry check. With an entry page, whatever page it is, that page is the entry checked first.
#[test]
fn nav3a_a_page_graph_package_declaring_a_prerequisite_without_a_page_entry_is_refused() {
    let detector = detector();
    let control = declaring("navigable_route");
    for entry in [None, Some("any")] {
        let error = program(entry)
            .required_entry(&control, &detector)
            .err()
            .unwrap_or_else(|| panic!("{entry:?}: the package is admitted"));
        assert_eq!(
            (error.code().as_str(), error.detail()),
            (
                ExecutionKernelCode::ContainedTaskControlInvalid.as_str(),
                Some("reason=prerequisite_requires_page_entry")
            ),
            "{entry:?}"
        );

        let undeclared = TaskControl {
            prerequisite_package_id: None,
            ..declaring("navigable_route")
        };
        let required = program(entry)
            .required_entry(&undeclared, &detector)
            .unwrap_or_else(|error| panic!("{entry:?} without a prerequisite: {error}"));
        assert!(required.is_none(), "{entry:?}: no entry check");
    }

    let required = program(Some("shop"))
        .required_entry(&control, &detector)
        .unwrap_or_else(|error| panic!("an entry page: {error}"))
        .expect("the entry page is checked first");
    assert_eq!(required.pages, ["neutral/shop"]);
}
