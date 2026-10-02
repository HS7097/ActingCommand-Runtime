// SPDX-License-Identifier: AGPL-3.0-only

//! One-off (to be reverted), Workflow #336 L2a evidence: the contained-task loader's own codes
//! for the linear packages (`PreparedContainedTask::load_path`, the path actingd admits a
//! package through) and the offline first decision of the linear packages that admit.
//! Every printed line starts with `KERNEL|`.

use actingcommand_contract::PackageRef;
use actingcommand_device::{CaptureBackendName, Frame};
use actingcommand_execution_kernel::{PreparedContainedTask, simulate_contained_task};
use std::path::Path;
use std::time::{Duration, Instant};

fn main() {
    let cases_path = std::env::args().nth(1).expect("cases.json path");
    let cases: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&cases_path).expect("read cases")).expect("cases JSON");
    let mut failures = 0_usize;
    for case in cases["admission"].as_array().expect("admission cases") {
        let name = case["name"].as_str().expect("name");
        let expect = case["expect"].as_str().expect("expect");
        let expect_detail = case["expect_detail"].as_str().unwrap_or("");
        let reference = PackageRef::parse_argument(case["reference"].as_str().expect("reference"))
            .expect("package reference");
        let result = PreparedContainedTask::load_path(
            "oneoff-336-l2a",
            Path::new(case["locator"].as_str().expect("locator")),
            &reference,
            None,
            Instant::now() + Duration::from_secs(60),
        );
        let (code, detail) = match &result {
            Ok(task) => (
                "ok".to_owned(),
                format!(
                    "package={} task={} mode={} maximum_executed_steps={} required_home_entry_page={:?}",
                    task.package_label(),
                    task.task_label(),
                    task.execution_mode(),
                    task.maximum_executed_steps(),
                    task.required_home_entry_page()
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
        let pass = code == expect && detail.contains(expect_detail);
        failures += usize::from(!pass);
        println!(
            "KERNEL|load_path|{name}|expect={expect} {expect_detail}|code={code}|{}|{detail}",
            if pass { "PASS" } else { "FAIL" }
        );
        if let (Ok(task), Some(frame)) = (&result, case["first_frame"].as_str()) {
            let frame = Frame::from_png(
                std::fs::read(frame).expect("frame PNG"),
                CaptureBackendName::FixtureSimulation,
            )
            .expect("frame");
            match simulate_contained_task(task, vec![frame]) {
                Ok(simulation) => {
                    let decision = serde_json::to_string(&simulation.decision).expect("decision JSON");
                    let pass = decision.contains(case["expect_decision"].as_str().unwrap_or("would_click"));
                    failures += usize::from(!pass);
                    println!(
                        "KERNEL|offline_first_decision|{name}|{}|{decision}|recognition={}",
                        if pass { "PASS" } else { "FAIL" },
                        serde_json::to_string(&simulation.recognition).expect("recognition JSON")
                    );
                }
                Err(error) => {
                    failures += 1;
                    println!("KERNEL|offline_first_decision|{name}|FAIL|{error}");
                }
            }
        }
    }
    println!("KERNEL|RESULT|failures={failures}");
    if failures > 0 {
        std::process::exit(1);
    }
}
