// SPDX-License-Identifier: AGPL-3.0-only

//! One-off (to be reverted), Workflow #336 L2b evidence: the contained-task loader's own codes and
//! details for the prerequisite declaration (`PreparedContainedTask::load_path`, the path actingd
//! admits a package through) and the new accessors on admitted packages. Every printed line
//! starts with `KERNEL|`.

use actingcommand_contract::PackageRef;
use actingcommand_execution_kernel::PreparedContainedTask;
use std::path::Path;
use std::time::{Duration, Instant};

fn load(
    case: &serde_json::Value,
) -> Result<PreparedContainedTask, actingcommand_execution_kernel::ContainedTaskError> {
    let reference = PackageRef::parse_argument(case["reference"].as_str().expect("reference"))
        .expect("package reference");
    PreparedContainedTask::load_path(
        "oneoff-336-l2b",
        Path::new(case["locator"].as_str().expect("locator")),
        &reference,
        None,
        Instant::now() + Duration::from_secs(60),
    )
}

fn main() {
    let cases_path = std::env::args().nth(1).expect("cases.json path");
    let cases: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&cases_path).expect("read cases"))
            .expect("cases JSON");
    let mut failures = 0_usize;
    for case in cases["admission"].as_array().expect("admission cases") {
        let name = case["name"].as_str().expect("name");
        let expect = case["expect"].as_str().expect("expect");
        let expect_detail = case["expect_detail"].as_str().unwrap_or("");
        let result = load(case);
        let (code, detail) = match &result {
            Ok(task) => (
                "ok".to_owned(),
                format!(
                    "package={} mode={} maximum_executed_steps={} required_home_entry_page={:?} linear_entry_page={:?} prerequisite={:?} compatible={} incompatibility={:?} entry_recovery_compatible={} resolution={:?}",
                    task.package_label(),
                    task.execution_mode(),
                    task.maximum_executed_steps(),
                    task.required_home_entry_page(),
                    task.linear_entry_page(),
                    task.prerequisite_package_id(),
                    task.is_prerequisite_compatible(),
                    task.prerequisite_incompatibility(),
                    task.is_entry_recovery_compatible(),
                    task.resolution()
                ),
            ),
            Err(error) => (
                error.code().to_owned(),
                format!(
                    "{} declaration_issue={}",
                    error.detail().unwrap_or("").replace('\n', " "),
                    error
                        .declaration_issue()
                        .map(|issue| serde_json::to_string(issue).expect("issue JSON"))
                        .unwrap_or_default()
                ),
            ),
        };
        let pass = expect == code && detail.contains(expect_detail);
        failures += usize::from(!pass);
        println!(
            "KERNEL|load_path|{name}|expect={expect} {expect_detail}|code={code}|{}|{detail}",
            if pass { "PASS" } else { "FAIL" }
        );
    }
    println!("KERNEL|RESULT|failures={failures}");
    if failures > 0 {
        std::process::exit(1);
    }
}
