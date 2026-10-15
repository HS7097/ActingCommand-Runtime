// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #373 NAV-3a (model M-373 v1.1 §4.2; test row N3a-1, exec key 67): a
// `navigable_route` package's control may declare `prerequisite_package_id`, as a `linear_steps`
// package's may; `in_page_guard` and `recognize_only` still may not.

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
