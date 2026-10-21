// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 slice E1 (train 0.12.9), T1 (A39): a package whose task declares no operation
// is refused as before (`package_invalid`, exit 2, `details.code` `contained_task_program_invalid`),
// and the refusal names the rule that refused it and where: `details.outcome` carries
// `program_problem` `operations_empty`, the task declaration's `path` inside the package and the
// refused member as `json_pointer`. Whether such a package should be admitted is not decided
// here. `package check` and `package preflight` both answer it. The tests for A40 and A41 are in
// `crates/resource-tooling/src/gate_411_e1.rs`.

use crate::package_tool::gate_390_tool1::neutral_package;
use serde_json::json;
use std::fs;
use tempfile::TempDir;

const TASK_FILE: &str = "resources/operations/task/task.json";
const PROGRAM_INVALID: &str = "contained_task_program_invalid";

#[test]
fn e1_t1_a_package_without_operations_is_refused_naming_its_rule_and_path() {
    let temp = TempDir::new().unwrap();
    let package = temp.path().join("neutral.zero.source");
    // The fixture package `package check` admits, with its one operation removed.
    let mut task = neutral_package(&package);
    task["operations"] = json!([]);
    fs::write(
        package.join(TASK_FILE),
        format!("{}\n", serde_json::to_string_pretty(&task).unwrap()),
    )
    .unwrap();

    for command in ["check", "preflight"] {
        let result = crate::run_cli(
            [
                "--json",
                "package",
                command,
                "--package",
                package.to_str().unwrap(),
            ],
            true,
        );
        let envelope = result.envelope_json();
        assert_eq!(result.exit_code(), 2, "{command}: {envelope}");
        let error = result
            .envelope
            .error
            .as_ref()
            .unwrap_or_else(|| panic!("{command}: not refused: {envelope}"));
        assert_eq!(error.code, "package_invalid", "{command}: {envelope}");
        let details = error
            .details
            .as_ref()
            .unwrap_or_else(|| panic!("{command}: a refusal without details: {envelope}"));
        assert_eq!(details["code"], PROGRAM_INVALID, "{command}: {envelope}");
        let outcome = &details["outcome"];
        assert_eq!(outcome["code"], PROGRAM_INVALID, "{command}: {envelope}");
        assert_eq!(
            outcome["values"]["program_problem"], "operations_empty",
            "{command}: {envelope}"
        );
        assert_eq!(
            outcome["values"]["path"], TASK_FILE,
            "{command}: {envelope}"
        );
        assert_eq!(
            outcome["values"]["json_pointer"], "/operations",
            "{command}: {envelope}"
        );
    }
}
