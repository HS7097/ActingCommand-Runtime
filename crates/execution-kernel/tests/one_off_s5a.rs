// SPDX-License-Identifier: AGPL-3.0-only

// one-off (to be reverted): Workflow #335 S5a evidence that compiles on the exact merge-base and
// on the PR head, so the two CI runs compare. P1: every pack of the public resource bundle,
// sealed ZIP admission and content-directory derivation. P5: the S-pack-3 shapes on both load
// paths. K6: trace sequences of packages without readings. K7: offline simulation of those
// packages. The CI log carries the ONE-OFF-S5A lines.

mod one_off_s5a_support;

use actingcommand_execution_kernel::{ContainedTaskTrace, simulate_contained_task};
use actingcommand_pack_containment::{Containment, InstanceId};
use actingcommand_recognition_pack::VisionProvider;
use one_off_s5a_support::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// A copy of `entries` whose `task` declaration is `value`, with its manifest hash updated.
fn with_task(
    entries: &BTreeMap<String, Vec<u8>>,
    task: &str,
    value: &Value,
) -> BTreeMap<String, Vec<u8>> {
    let mut entries = entries.clone();
    let bytes = json_bytes(value);
    if let Some(manifest) = entries.get("resources/manifest.json").cloned() {
        let mut manifest: Value = serde_json::from_slice(&manifest).expect("manifest JSON");
        let relative = format!("operations/{task}/task.json");
        if let Some(files) = manifest["files"].as_array_mut() {
            for file in files {
                if file["path"] == relative.as_str() {
                    file["sha256"] = json!(format!("sha256:{}", sha256_hex(&bytes)));
                }
            }
        }
        entries.insert("resources/manifest.json".to_owned(), json_bytes(&manifest));
    }
    entries.insert(task_path(task), bytes);
    entries
}

/// The S-pack-3 shape of a bundle task: a credits reading on home, and for a task without the
/// credits OCR target, that target copied from the observation package.
fn s_pack_3(task: &str, entries: &BTreeMap<String, Vec<u8>>) -> Value {
    let mut value: Value = serde_json::from_slice(&entries[&task_path(task)]).expect("task JSON");
    value["resource_readings"] = json!([reading("home")]);
    let declares_target = value["ocr_targets"]
        .as_array()
        .is_some_and(|targets| targets.iter().any(|target| target["id"] == "ocr/credits"));
    if !declares_target {
        let observation = read_zip(&bundle_pack("home_readings"));
        let observation: Value =
            serde_json::from_slice(&observation[&task_path("home_readings")]).expect("task JSON");
        let target = observation["ocr_targets"]
            .as_array()
            .expect("ocr targets")
            .iter()
            .find(|target| target["id"] == "ocr/credits")
            .expect("credits target")
            .clone();
        value["ocr_targets"] = json!([target]);
    }
    value
}

#[test]
fn one_off_s5a_p1_bundle_packs() {
    for (name, bytes) in bundle_packs() {
        report(
            "P1",
            format!(
                "pack {name} bytes={} sha256={}",
                bytes.len(),
                sha256_hex(&bytes)
            ),
        );
        report(
            "P1",
            format!("zip {name} {}", describe_load(&load_zip(&bytes, None))),
        );
        let source = unsealed(&read_zip(&bytes));
        let directory = write_content_directory("p1", &source);
        let reference = content_reference(&source);
        let mut containment = Containment::new();
        let instance = InstanceId::new("one-off-s5a").expect("instance");
        match containment.load_path(&instance, &directory, &reference, false, deadline()) {
            Ok(bundle) => {
                let mut paths = bundle.entry_paths().map(str::to_owned).collect::<Vec<_>>();
                paths.sort();
                for path in paths {
                    let bytes = bundle.entry(&path).unwrap_or_default();
                    report("P1", format!("derived {name} {path} {}", sha256_hex(bytes)));
                }
            }
            Err(error) => report("P1", format!("derived {name} refused {error}")),
        }
        report(
            "P1",
            format!(
                "directory {name} {}",
                describe_load(&load_directory("p1", &source, None))
            ),
        );
    }
}

#[test]
fn one_off_s5a_p5_s_pack_3_shapes_on_both_paths() {
    for task in ["home_readings", "cafe_income"] {
        let sealed = read_zip(&bundle_pack(task));
        let value = s_pack_3(task, &sealed);
        report(
            "P5",
            format!("{task} resource_readings={}", value["resource_readings"]),
        );
        let zip = zip_entries(&with_task(&sealed, task, &value));
        report(
            "P5",
            format!("zip {task} {}", describe_load(&load_zip(&zip, None))),
        );
        let directory = with_task(&unsealed(&sealed), task, &value);
        report(
            "P5",
            format!(
                "directory {task} {}",
                describe_load(&load_directory("p5", &directory, None))
            ),
        );
    }
}

#[test]
fn one_off_s5a_k6_k7_packages_without_readings() {
    let navigable = load_zip(&navigable_package(), None).expect("navigable package");
    let first = run_and_report(
        "K6 navigable",
        &navigable,
        vec![frame(HOME, EMPTY), frame(DONE, BLACK)],
    );
    let replay = run_and_report(
        "K6 navigable replay",
        &navigable,
        vec![frame(HOME, EMPTY), frame(DONE, BLACK)],
    );
    report(
        "K6",
        format!(
            "navigable replay identical={}",
            first.fingerprint() == replay.fingerprint()
        ),
    );

    let stub = StubOcr::with(&[("238,334,214", Some(0.97))]);
    let provider: Arc<dyn VisionProvider> = stub.clone();
    let fields = load_zip(&fields_package(None), Some(provider)).expect("fields package");
    let runtime = run_and_report("K6 fields", &fields, vec![frame(HOME, BLACK)]);
    report("K6", format!("fields ocr_calls={}", stub.calls()));
    for trace in &runtime.traces {
        if let ContainedTaskTrace::PostAdmissionOcrFields {
            report: fields_report,
        } = trace
        {
            let bytes = serde_json::to_vec(fields_report).expect("fields report JSON");
            report(
                "K4",
                format!(
                    "fields report sha256={} json={}",
                    sha256_hex(&bytes),
                    String::from_utf8_lossy(&bytes)
                ),
            );
        }
    }

    let claim = load_zip(&claim_package(None), None).expect("claim package");
    run_and_report(
        "K6 claim",
        &claim,
        vec![frame(HOME, BLACK), frame(DONE, BLACK)],
    );

    let fields_offline = load_zip(&fields_package(None), None).expect("fields package");
    for (label, task, frames) in [
        ("navigable home", &navigable, vec![frame(HOME, EMPTY)]),
        ("navigable terminal", &navigable, vec![frame(DONE, BLACK)]),
        ("fields home", &fields_offline, vec![frame(HOME, BLACK)]),
        ("claim home", &claim, vec![frame(HOME, BLACK)]),
        ("claim done", &claim, vec![frame(DONE, BLACK)]),
    ] {
        match simulate_contained_task(task, frames) {
            Ok(result) => report(
                "K7",
                format!(
                    "{label} {}",
                    serde_json::to_string(&result).expect("simulation JSON")
                ),
            ),
            Err(error) => report("K7", format!("{label} error {error}")),
        }
    }
}
