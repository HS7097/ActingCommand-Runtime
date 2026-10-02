// SPDX-License-Identifier: AGPL-3.0-only

//! One-off (to be reverted), Workflow #336 L1 evidence: the contained-task loader's own codes
//! for every container case (`PreparedContainedTask::load_path`, the path actingd admits a
//! procedure package through), and the in-memory route (`expand_content_container` then
//! `PreparedContainedTask::load_content_entries`). Every printed line starts with `KERNEL|`.

use actingcommand_contract::{ContentDirectory, ContentDirectoryVersion, PackageRef};
use actingcommand_execution_kernel::PreparedContainedTask;
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use std::path::Path;
use std::time::{Duration, Instant};

fn content(sha256: &str) -> ContentDirectory {
    ContentDirectory {
        schema_version: ContentDirectoryVersion::V1,
        sha256: sha256.to_owned(),
    }
}

fn outcome(result: &Result<PreparedContainedTask, actingcommand_execution_kernel::ContainedTaskError>) -> (String, String) {
    match result {
        Ok(task) => (
            "ok".to_owned(),
            format!(
                "package={} task={} mode={} reference={}",
                task.package_label(),
                task.task_label(),
                task.execution_mode(),
                serde_json::to_string(task.package_sha256()).expect("reference JSON")
            ),
        ),
        Err(error) => (
            error.code().to_owned(),
            error.detail().unwrap_or("").replace('\n', " "),
        ),
    }
}

fn main() {
    let cases_path = std::env::args().nth(1).expect("cases.json path");
    let cases: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&cases_path).expect("read cases")).expect("cases JSON");
    let mut failures = 0_usize;
    for case in cases["load"].as_array().expect("load cases") {
        let name = case["name"].as_str().expect("name");
        let expect = case["expect"].as_str().expect("expect");
        let reference = PackageRef::ContentDirectory(content(case["sha256"].as_str().expect("sha256")));
        let result = PreparedContainedTask::load_path(
            "oneoff-336-l1",
            Path::new(case["locator"].as_str().expect("locator")),
            &reference,
            None,
            Instant::now() + Duration::from_secs(60),
        );
        let (code, detail) = outcome(&result);
        let pass = code == expect;
        failures += usize::from(!pass);
        println!("KERNEL|load_path|{name}|expect={expect}|code={code}|{}|{detail}", if pass { "PASS" } else { "FAIL" });
    }
    for case in cases["entries"].as_array().expect("entry cases") {
        let name = case["name"].as_str().expect("name");
        let expect = case["expect"].as_str().expect("expect");
        let container = match case["kind"].as_str().expect("kind") {
            "zip" => ContentContainer::Zip,
            _ => ContentContainer::Json,
        };
        let bytes = std::fs::read(case["file"].as_str().expect("file")).expect("container bytes");
        let entries = match expand_content_container(&bytes, container, ContainmentLimits::default()) {
            Ok(entries) => entries,
            Err(error) => {
                failures += 1;
                println!("KERNEL|expand|{name}|FAIL|{error}");
                continue;
            }
        };
        println!(
            "KERNEL|expand|{name}|entries={}|bytes={}|paths={}",
            entries.len(),
            entries.values().map(Vec::len).sum::<usize>(),
            entries.keys().cloned().collect::<Vec<_>>().join(",")
        );
        let result = PreparedContainedTask::load_content_entries(
            "oneoff-336-l1",
            entries,
            &content(case["sha256"].as_str().expect("sha256")),
            None,
            Instant::now() + Duration::from_secs(60),
        );
        let (code, detail) = outcome(&result);
        let pass = code == expect;
        failures += usize::from(!pass);
        println!("KERNEL|load_content_entries|{name}|expect={expect}|code={code}|{}|{detail}", if pass { "PASS" } else { "FAIL" });
    }
    println!("KERNEL|RESULT|failures={failures}");
    if failures > 0 {
        std::process::exit(1);
    }
}
