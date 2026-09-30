// SPDX-License-Identifier: AGPL-3.0-only

// one-off (to be reverted): Workflow #335 S5a evidence on the PR head only. K1-K5 and K7 for
// packages with readings, P2 (the S-pack-3 shapes admitted on both load paths) and P3
// (authoring mistakes with their reason and field pointer). The CI log carries the
// ONE-OFF-S5A lines.

mod one_off_s5a_support;

use actingcommand_execution_kernel::{ContainedTaskTrace, simulate_contained_task};
use actingcommand_pack_containment::{Containment, InstanceId};
use actingcommand_recognition_pack::VisionProvider;
use one_off_s5a_support::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

fn provider(stub: &Arc<StubOcr>) -> Option<Arc<dyn VisionProvider>> {
    let provider: Arc<dyn VisionProvider> = stub.clone();
    Some(provider)
}

fn report_readings(tag: &str, runtime: &TraceRuntime, captured_at: Option<SystemTime>) {
    let names = runtime.names();
    let readings_at = names.iter().position(|name| name == "ResourceReadings");
    let finalizing_at = names.iter().position(|name| name == "Finalizing");
    report(
        tag,
        format!("readings_position={readings_at:?} finalizing_position={finalizing_at:?}"),
    );
    for trace in &runtime.traces {
        if let ContainedTaskTrace::ResourceReadings {
            captured_at: trace_time,
            readings,
        } = trace
        {
            report(
                tag,
                format!(
                    "trace captured_at_equals_terminal_frame={:?} readings={:?}",
                    captured_at.map(|time| time == *trace_time),
                    readings
                        .iter()
                        .map(|reading| (
                            reading.declaration.id.as_str(),
                            reading.declaration.fact_key.as_str(),
                            reading.value,
                            reading.confidence_milli
                        ))
                        .collect::<Vec<_>>()
                ),
            );
        }
    }
}

fn mutate(base: &Value, change: impl FnOnce(&mut Value)) -> Value {
    let mut value = base.clone();
    change(&mut value);
    value
}

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
fn one_off_s5a_k1_k2_k3_claim_with_readings() {
    let with_readings = claim_package(Some(json!([reading("done")])));

    let stub = StubOcr::with(&[("238,334,214", Some(0.97))]);
    let task = load_zip(&with_readings, provider(&stub)).expect("claim package with readings");
    let terminal = frame(DONE, BLACK);
    let captured_at = terminal.captured_at;
    let runtime = run_and_report("K1", &task, vec![frame(HOME, BLACK), terminal]);
    report_readings("K1", &runtime, Some(captured_at));
    report("K1", format!("ocr_calls={}", stub.calls()));
    let plain = load_zip(&claim_package(None), None).expect("claim package");
    let baseline = run_and_report(
        "K1 without readings",
        &plain,
        vec![frame(HOME, BLACK), frame(DONE, BLACK)],
    );
    report(
        "K1",
        format!(
            "captures with_readings={} without={}; inputs with_readings={} without={}",
            runtime.captures, baseline.captures, runtime.inputs, baseline.inputs
        ),
    );

    let stub = StubOcr::with(&[]);
    let task = load_zip(&with_readings, provider(&stub)).expect("claim package with readings");
    let runtime = run_and_report("K2", &task, vec![frame(HOME, BLACK), frame(EMPTY, BLACK)]);
    report_readings("K2", &runtime, None);
    report("K2", format!("ocr_calls={}", stub.calls()));

    for (label, text, confidence) in [
        ("empty", "", Some(0.97_f32)),
        ("comma lost", "238334214", Some(0.97)),
        ("confidence 0.85 below 900", "238,334,214", Some(0.85)),
    ] {
        let stub = StubOcr::with(&[(text, confidence)]);
        let task = load_zip(&with_readings, provider(&stub)).expect("claim package with readings");
        let tag = format!("K3 {label}");
        let runtime = run_and_report(&tag, &task, vec![frame(HOME, BLACK), frame(DONE, BLACK)]);
        report_readings(&tag, &runtime, None);
        report(
            &tag,
            format!("ocr_calls={} inputs={}", stub.calls(), runtime.inputs),
        );
    }
}

#[test]
fn one_off_s5a_k4_fields_and_reading_on_one_frame() {
    let stub = StubOcr::with(&[("238,334,214", Some(0.97)), ("238,334,214", Some(0.97))]);
    let task = load_zip(
        &fields_package(Some(json!([reading("home")]))),
        provider(&stub),
    )
    .expect("fields package with readings");
    let terminal = frame(HOME, BLACK);
    let captured_at = terminal.captured_at;
    let runtime = run_and_report("K4", &task, vec![terminal]);
    report("K4", format!("ocr_calls={}", stub.calls()));
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
    report_readings("K4", &runtime, Some(captured_at));
}

#[test]
fn one_off_s5a_k5_author_mistakes_refused_before_input() {
    let readings = Some(json!([reading("done")]));
    let stub = StubOcr::with(&[]);

    let mut pages = claim_pages();
    pages[1]["optional"] = json!(["ocr/credits"]);
    let gated = zip_entries(&claim_entries(readings.clone(), &pages));
    report(
        "K5",
        format!(
            "target in a page gate: {}",
            describe_load(&load_zip(&gated, provider(&stub)))
        ),
    );

    for privacy in ["personal", "public"] {
        let mut entries = claim_entries(readings.clone(), &claim_pages());
        entries.insert(
            "resources/navigation/neutral.test.navigation.json".to_owned(),
            json_bytes(&json!({
                "schema_version": "0.6",
                "converter_schema_version": "0.5",
                "generated": true,
                "generated_by": "actinglab resource convert",
                "game": "neutral",
                "server": "test",
                "coordinate_space": {"width": 2, "height": 1},
                "control_points": [],
                "navigation": [],
                "page_operations": [],
                "destructive_actions": []
            })),
        );
        entries.insert(
            "resources/navigation/neutral.test.projection.json".to_owned(),
            json_bytes(&json!({
                "schema_version": "actingcommand.page-projection-metadata.v1",
                "actions": [],
                "targets": [{"target_id": "ocr/credits", "privacy": privacy, "source": "fixture/owner"}],
                "fields": [],
                "pages": []
            })),
        );
        let files = [
            "navigation/neutral.test.navigation.json",
            "navigation/neutral.test.projection.json",
            "recognition/neutral.test.pack.json",
            "recognition/neutral.test.pages.json",
        ]
        .iter()
        .map(|path| {
            json!({
                "path": path,
                "sha256": sha256_hex(&entries[&format!("resources/{path}")])
            })
        })
        .collect::<Vec<_>>();
        entries.insert(
            "resources/manifest.json".to_owned(),
            json_bytes(&json!({"schema_version": "0.3", "entry_task_id": "task", "files": files})),
        );
        report(
            "K5",
            format!(
                "target marked {privacy}: {}",
                describe_load(&load_zip(&zip_entries(&entries), provider(&stub)))
            ),
        );
    }
    report(
        "K5",
        format!(
            "ocr_calls={} (admission refuses before any run, so no capture and no input)",
            stub.calls()
        ),
    );
}

#[test]
fn one_off_s5a_k7_offline_simulation_with_readings() {
    for with_provider in [false, true] {
        let stub = StubOcr::with(&[]);
        for (label, bytes, frames) in [
            (
                "fields with readings",
                fields_package(Some(json!([reading("home")]))),
                vec![frame(HOME, BLACK)],
            ),
            (
                "claim with readings, terminal done",
                claim_package(Some(json!([reading("done")]))),
                vec![frame(DONE, BLACK)],
            ),
        ] {
            let task = load_zip(&bytes, with_provider.then(|| provider(&stub)).flatten())
                .expect("package with readings");
            match simulate_contained_task(&task, frames) {
                Ok(result) => report(
                    "K7",
                    format!(
                        "{label} provider={with_provider} {}",
                        serde_json::to_string(&result).expect("simulation JSON")
                    ),
                ),
                Err(error) => report("K7", format!("{label} error {error}")),
            }
        }
        report(
            "K7",
            format!("provider={with_provider} ocr_calls={}", stub.calls()),
        );
    }
}

#[test]
fn one_off_s5a_p2_s_pack_3_shapes_admitted() {
    for task in ["home_readings", "cafe_income"] {
        let sealed = read_zip(&bundle_pack(task));
        let value = s_pack_3(task, &sealed);
        let source = with_task(&unsealed(&sealed), task, &value);
        let directory = write_content_directory("p2", &source);
        let reference = content_reference(&source);
        let mut containment = Containment::new();
        let instance = InstanceId::new("one-off-s5a").expect("instance");
        match containment.load_path(&instance, &directory, &reference, false, deadline()) {
            Ok(bundle) => {
                report(
                    "P2",
                    format!(
                        "directory {task} compiled operation resource_readings={} ocr_targets={}",
                        bundle.operation()["resource_readings"],
                        bundle.operation()["ocr_targets"]
                    ),
                );
                let compiled = bundle
                    .entry_paths()
                    .map(|path| {
                        (
                            path.to_owned(),
                            bundle.entry(path).unwrap_or_default().to_vec(),
                        )
                    })
                    .collect::<BTreeMap<_, _>>();
                report(
                    "P2",
                    format!(
                        "zip of the compiled {task} {}",
                        describe_load(&load_zip(&zip_entries(&compiled), None))
                    ),
                );
            }
            Err(error) => report("P2", format!("directory {task} refused {error}")),
        }
        report(
            "P2",
            format!(
                "directory {task} {}",
                describe_load(&load_directory("p2", &source, None))
            ),
        );
        report(
            "P2",
            format!(
                "sealed zip {task} with the member {}",
                describe_load(&load_zip(
                    &zip_entries(&with_task(&sealed, task, &value)),
                    None
                ))
            ),
        );
    }
}

#[test]
fn one_off_s5a_p3_authoring_mistakes() {
    let sealed_home = read_zip(&bundle_pack("home_readings"));
    let home = s_pack_3("home_readings", &sealed_home);
    let sealed_cafe = read_zip(&bundle_pack("cafe_income"));
    let cafe = s_pack_3("cafe_income", &sealed_cafe);
    let cases = vec![
        (
            "schema 0.7 task declares readings",
            "home_readings",
            mutate(&home, |value| value["schema_version"] = json!("0.7")),
        ),
        (
            "unknown key ttl_ms",
            "home_readings",
            mutate(&home, |value| {
                value["resource_readings"][0]["ttl_ms"] = json!(600000)
            }),
        ),
        (
            "max copied as u64::MAX",
            "home_readings",
            mutate(&home, |value| {
                value["resource_readings"][0]["value"]["max"] = json!(u64::MAX)
            }),
        ),
        (
            "fact_key written as credits",
            "home_readings",
            mutate(&home, |value| {
                value["resource_readings"][0]["fact_key"] = json!("credits")
            }),
        ),
        (
            "page is not a terminal page",
            "cafe_income",
            mutate(&cafe, |value| {
                value["resource_readings"][0]["page_id"] = json!("cafe")
            }),
        ),
        (
            "target is a template id",
            "home_readings",
            mutate(&home, |value| {
                value["resource_readings"][0]["target_id"] = json!("page/home")
            }),
        ),
        (
            "scheduling_outcome missing",
            "home_readings",
            mutate(&home, |value| {
                value
                    .as_object_mut()
                    .expect("task object")
                    .remove("scheduling_outcome");
            }),
        ),
        (
            "same fact_key twice",
            "home_readings",
            mutate(&home, |value| {
                let mut second = value["resource_readings"][0].clone();
                second["id"] = json!("credits_again");
                value["resource_readings"]
                    .as_array_mut()
                    .expect("readings")
                    .push(second);
            }),
        ),
        (
            "valid_for_ms zero",
            "home_readings",
            mutate(&home, |value| {
                value["resource_readings"][0]["valid_for_ms"] = json!(0)
            }),
        ),
        (
            "minimum_confidence_milli zero",
            "home_readings",
            mutate(&home, |value| {
                value["resource_readings"][0]["minimum_confidence_milli"] = json!(0)
            }),
        ),
    ];
    for (label, task, value) in cases {
        let sealed = if task == "home_readings" {
            &sealed_home
        } else {
            &sealed_cafe
        };
        report(
            "P3",
            format!(
                "{label}: directory {}",
                describe_load(&load_directory(
                    "p3",
                    &with_task(&unsealed(sealed), task, &value),
                    None
                ))
            ),
        );
        report(
            "P3",
            format!(
                "{label}: sealed zip {}",
                describe_load(&load_zip(
                    &zip_entries(&with_task(sealed, task, &value)),
                    None
                ))
            ),
        );
    }
}
