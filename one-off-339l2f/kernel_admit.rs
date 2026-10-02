// SPDX-License-Identifier: AGPL-3.0-only

//! One-off (to be reverted), Workflow #339 L2f evidence E12: an older build's own loader
//! (`PreparedContainedTask::load_path`, the path actingd admits a package through) on the
//! optional-step package. Only API that v0.9.1 and the L2a and L2e sources share. Every printed
//! line starts with `ADMIT|`.

use actingcommand_contract::PackageRef;
use actingcommand_execution_kernel::PreparedContainedTask;
use std::path::Path;
use std::time::{Duration, Instant};

fn main() {
    let label = std::env::args().nth(1).expect("build label");
    let cases_path = std::env::args().nth(2).expect("cases.json path");
    let cases: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&cases_path).expect("read cases"))
            .expect("cases JSON");
    let mut failures = 0_usize;
    for case in cases["admission"].as_array().into_iter().flatten() {
        let name = case["name"].as_str().expect("name");
        let expect = case["expect"].as_str().expect("expect");
        let expect_detail = case["expect_detail"].as_str().unwrap_or("");
        let reference = PackageRef::parse_argument(case["reference"].as_str().expect("reference"))
            .expect("package reference");
        let result = PreparedContainedTask::load_path(
            "oneoff-339-l2f",
            Path::new(case["locator"].as_str().expect("locator")),
            &reference,
            None,
            Instant::now() + Duration::from_secs(60),
        );
        let (code, detail, issue) = match &result {
            Ok(task) => (
                "ok".to_owned(),
                format!("mode={}", task.execution_mode()),
                String::new(),
            ),
            Err(error) => (
                error.code().to_owned(),
                error.detail().unwrap_or("").replace('\n', " "),
                error
                    .declaration_issue()
                    .map(|issue| serde_json::to_string(issue).expect("issue JSON"))
                    .unwrap_or_default(),
            ),
        };
        let pass = code == expect
            && expect_detail
                .split(" && ")
                .all(|part| issue.contains(part));
        failures += usize::from(!pass);
        println!(
            "ADMIT|{label}|{name}|expect={expect} {expect_detail}|code={code}|{}|detail={detail}|declaration_issue={issue}",
            if pass { "PASS" } else { "FAIL" }
        );
    }
    println!("ADMIT|{label}|RESULT|failures={failures}");
    if failures > 0 {
        std::process::exit(1);
    }
}
