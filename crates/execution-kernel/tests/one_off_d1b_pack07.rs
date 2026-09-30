// SPDX-License-Identifier: AGPL-3.0-only

//! One-off evidence for Workflow #308 D1b, to be reverted. Copies of published task packages
//! gain a schema 0.7 pack with a color digest, composites (digest and template, digest and
//! OCR) and per-target color thresholds, are admitted through the production loader, and
//! are judged on frames composed of the bundle's own images at their declared rectangles:
//! the home screen, the home screen under a reminder popup, and the cafe screen. The digest
//! is authored the v0.9 way: a draft entry is observed on the home frame and its observed
//! cells become the declaration.

use actingcommand_contract::InputAction;
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_execution_kernel::{
    ContainedTaskRuntime, ContainedTaskTrace, ExternalExpectedSha256, ExternallyVerifiedBundle,
    InputFrameContext, ObservedFrame, PreparedContainedTask,
};
use actingcommand_pack_containment::Sha256Hash;
use actingcommand_recognition::Scene;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::process::Command;

type Entries = BTreeMap<String, Vec<u8>>;

const ARCHIVE_URL: &str =
    "https://github.com/HS7097/ActingCommand/archive/536f048a3cac26ddbb0391895391f98967704cb0.zip";
const BUNDLE_SHA256: &str = "df524eac6290c4c0b435f3872a89e12dbe4f49e37555fa102bf96dd71b696af8";
const DIGEST_REGION: (i32, i32, i32, i32) = (77, 680, 49, 18);

fn emit(line: &str) {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "D1B-PACK07 {}", line.replace('\n', " | "))
        .unwrap_or_else(|error| panic!("write stdout: {error}"));
    stdout
        .flush()
        .unwrap_or_else(|error| panic!("flush stdout: {error}"));
}

fn download_archive() -> Vec<u8> {
    let output = Command::new("curl")
        .args(["-sSfL", "--retry", "3", ARCHIVE_URL])
        .output()
        .unwrap_or_else(|error| panic!("start curl: {error}"));
    assert!(
        output.status.success(),
        "curl failed: {} {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn unzip(bytes: &[u8]) -> Entries {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .unwrap_or_else(|error| panic!("open zip: {error}"));
    let mut entries = Entries::new();
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .unwrap_or_else(|error| panic!("zip entry {index}: {error}"));
        if file.is_dir() {
            continue;
        }
        let name = file.name().to_owned();
        let mut data = Vec::new();
        file.read_to_end(&mut data)
            .unwrap_or_else(|error| panic!("read {name}: {error}"));
        entries.insert(name, data);
    }
    entries
}

fn zip_entries(entries: &Entries) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, data) in entries {
        writer
            .start_file(name.as_str(), options)
            .unwrap_or_else(|error| panic!("zip {name}: {error}"));
        writer
            .write_all(data)
            .unwrap_or_else(|error| panic!("zip {name}: {error}"));
    }
    writer
        .finish()
        .unwrap_or_else(|error| panic!("finish zip: {error}"))
        .into_inner()
}

fn entry_path(entries: &Entries, suffix: &str) -> String {
    entries
        .keys()
        .find(|path| path.ends_with(suffix))
        .unwrap_or_else(|| panic!("no entry ending with {suffix}"))
        .clone()
}

fn json_entry(entries: &Entries, suffix: &str) -> Value {
    let path = entry_path(entries, suffix);
    serde_json::from_slice(&entries[&path]).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn expected(zip: &[u8]) -> ExternalExpectedSha256 {
    ExternalExpectedSha256::parse_hex(&Sha256Hash::digest(zip).to_string())
        .unwrap_or_else(|error| panic!("{error:?}"))
}

/// The sealed package whose file name ends with `suffix`, checked against `bundle.json`.
fn package(bundle: &Entries, suffix: &str) -> Entries {
    let index: Value = serde_json::from_slice(&bundle["bundle.json"]).expect("bundle.json");
    let pack = index["packs"]
        .as_array()
        .expect("packs")
        .iter()
        .find(|pack| {
            pack["path"]
                .as_str()
                .is_some_and(|path| path.ends_with(suffix))
        })
        .unwrap_or_else(|| panic!("no package ending with {suffix}"));
    let path = pack["path"].as_str().expect("path");
    let bytes = &bundle[path];
    assert_eq!(
        Sha256Hash::digest(bytes).to_string(),
        pack["sha256"].as_str().expect("sha256"),
        "{path}"
    );
    unzip(bytes)
}

fn target<'a>(pack: &'a Value, id: &str) -> &'a Value {
    pack["targets"]
        .as_array()
        .expect("targets")
        .iter()
        .find(|target| target["id"] == id)
        .unwrap_or_else(|| panic!("target {id} missing"))
}

/// A 1280x720 frame showing, for each `(package, target)`, the bundle's own image of that
/// template at the rectangle the package declares for it; every other pixel is black.
fn compose(parts: &[(&Entries, &str)]) -> (Scene, Vec<u8>) {
    let (width, height) = (1280_usize, 720_usize);
    let mut canvas = vec![0_u8; width * height * 3];
    for (entries, id) in parts {
        let pack = json_entry(entries, ".pack.json");
        let declared = target(&pack, id);
        let path = declared["template_path"].as_str().expect("template_path");
        let crop = Scene::from_png(&entries[&format!("resources/{path}")])
            .unwrap_or_else(|error| panic!("{path}: {error}"));
        let x = declared["region"]["x"].as_u64().expect("x") as usize;
        let y = declared["region"]["y"].as_u64().expect("y") as usize;
        let crop_width = crop.width() as usize;
        assert_eq!(
            declared["region"]["width"].as_u64(),
            Some(crop_width as u64)
        );
        for row in 0..crop.height() as usize {
            let source = &crop.rgb8_pixels()[row * crop_width * 3..(row + 1) * crop_width * 3];
            let start = ((y + row) * width + x) * 3;
            canvas[start..start + crop_width * 3].copy_from_slice(source);
        }
    }
    let scene = Scene::from_rgb8(width as u32, height as u32, &canvas)
        .unwrap_or_else(|error| panic!("{error}"));
    (scene, canvas)
}

/// The package with its derived pack and page set replaced and the manifest hashes renewed.
fn rebuild(entries: &Entries, pack: &Value, pages: &Value) -> Vec<u8> {
    let mut package = entries.clone();
    let pack_path = entry_path(entries, ".pack.json");
    let pages_path = entry_path(entries, ".pages.json");
    package.insert(pack_path.clone(), serde_json::to_vec(pack).expect("pack"));
    package.insert(
        pages_path.clone(),
        serde_json::to_vec(pages).expect("pages"),
    );
    let mut manifest = json_entry(entries, "resources/manifest.json");
    for file in manifest["files"].as_array_mut().expect("manifest files") {
        let path = format!("resources/{}", file["path"].as_str().expect("file path"));
        if path == pack_path || path == pages_path {
            file["sha256"] = json!(format!("sha256:{}", Sha256Hash::digest(&package[&path])));
        }
    }
    package.insert(
        "resources/manifest.json".to_owned(),
        serde_json::to_vec(&manifest).expect("manifest"),
    );
    zip_entries(&package)
}

fn admit(zip: &[u8]) -> Result<PreparedContainedTask, String> {
    PreparedContainedTask::load("one-off.instance", zip, expected(zip))
        .map_err(|error| format!("{error:?}"))
}

fn verdict(result: &Result<PreparedContainedTask, String>) -> String {
    match result {
        Ok(_) => "ok".to_owned(),
        Err(error) => error.clone(),
    }
}

fn loaded(zip: &[u8]) -> ExternallyVerifiedBundle {
    ExternallyVerifiedBundle::load("one-off.instance", zip, expected(zip))
        .unwrap_or_else(|error| panic!("load: {error}"))
}

fn digest_target(cells: &str) -> Value {
    let (x, y, width, height) = DIGEST_REGION;
    json!({
        "type": "color_digest",
        "id": "digest/home_cafe",
        "region": {"x": x, "y": y, "width": width, "height": height},
        "algorithm": "color_digest.v1",
        "columns": 8,
        "rows": 8,
        "cells": cells,
        "exclude_cells": [],
        "max_mean_milli": 1500,
        "max_cell": 12
    })
}

fn color_target(id: &str, mean: [u8; 3], max_distance: Option<f32>) -> Value {
    let (x, y, width, height) = DIGEST_REGION;
    let mut target = json!({
        "type": "color",
        "id": id,
        "region": {"x": x, "y": y, "width": width, "height": height},
        "expected": mean
    });
    if let Some(max_distance) = max_distance {
        target["max_distance"] = json!(max_distance);
    }
    target
}

fn composite(id: &str, mode: &str, members: &[&str]) -> Value {
    json!({"type": "composite", "id": id, "mode": mode, "members": members})
}

fn push(pack: &mut Value, target: Value) {
    pack["targets"]
        .as_array_mut()
        .expect("targets")
        .push(target);
}

fn summary(evaluation: &actingcommand_recognition_pack::TargetEvaluation) -> String {
    let mut text = format!(
        "{} kind={:?} passed={}",
        evaluation.id, evaluation.kind, evaluation.passed
    );
    if let Some(digest) = &evaluation.color_digest {
        text.push_str(&format!(
            " digest(active_cells={} mean_milli={} max_cell={} worst_cell={} max_mean_milli={} max_cell_threshold={:?})",
            digest.active_cells,
            digest.mean_milli,
            digest.max_cell,
            digest.worst_cell,
            digest.max_mean_milli,
            digest.max_cell_threshold
        ));
    }
    if let Some(color) = &evaluation.color {
        text.push_str(&format!(
            " color(distance={:.3} max_distance={} mean={:?})",
            color.distance, color.max_distance, color.mean
        ));
    }
    if let Some(template) = &evaluation.template {
        text.push_str(&format!(" template(score={:.4})", template.score));
    }
    if let Some(composite) = &evaluation.composite {
        let members = composite
            .members
            .iter()
            .map(|member| format!("{}={}", member.target_id, member.passed))
            .collect::<Vec<_>>()
            .join(",");
        text.push_str(&format!(" {:?}[{members}]", composite.mode));
    }
    text.push_str(&format!(" message={:?}", evaluation.message));
    text
}

struct FrameRuntime {
    frame: Option<Frame>,
    recognized: Vec<String>,
}

impl ContainedTaskRuntime for FrameRuntime {
    type Error = String;

    fn capture(&mut self) -> Result<ObservedFrame, String> {
        self.frame
            .take()
            .map(ObservedFrame::from)
            .ok_or_else(|| "one frame only".to_owned())
    }

    fn input(
        &mut self,
        _action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), String> {
        Err("recognition takes no input".to_owned())
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), String> {
        if let ContainedTaskTrace::RecognitionCompleted {
            page_label,
            targets,
            ..
        } = trace
        {
            self.recognized.push(format!(
                "page_label={page_label:?} targets={}",
                serde_json::to_string(&targets).expect("targets")
            ));
        }
        Ok(())
    }
}

fn kernel_home(task: &PreparedContainedTask, canvas: &[u8]) -> String {
    let frame = Frame::from_pixels(
        1280,
        720,
        canvas.to_vec(),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .unwrap_or_else(|error| panic!("frame: {error}"));
    let mut runtime = FrameRuntime {
        frame: Some(frame),
        recognized: Vec::new(),
    };
    let matched = task.recognize_required_home(&mut runtime);
    format!("matched={matched:?} {}", runtime.recognized.join(" "))
}

#[test]
fn one_off_d1b_pack07_digest_and_composites_on_bundle_frames() {
    let archive = unzip(&download_archive());
    let (bundle_name, bundle_bytes) = archive
        .iter()
        .find(|(path, _)| path.contains("/bundles/") && path.ends_with(".zip"))
        .unwrap_or_else(|| panic!("no bundle in the archive"));
    assert_eq!(
        Sha256Hash::digest(bundle_bytes).to_string(),
        BUNDLE_SHA256,
        "{bundle_name}"
    );
    let bundle = unzip(bundle_bytes);
    let cafe = package(&bundle, ".cafe_income.zip");
    let notice = package(&bundle, ".notice_home.zip");
    let readings = package(&bundle, ".home_readings.zip");

    let frames = [
        (
            "home",
            compose(&[(&cafe, "page/home"), (&cafe, "ui/home_work_label")]),
        ),
        (
            "home_under_reminder",
            compose(&[
                (&notice, "ui/event_reminder_hud_cafe"),
                (&notice, "ui/event_reminder_hud_work"),
                (&notice, "ui/event_reminder_header"),
                (&notice, "page/event_reminder_visible"),
                (&notice, "ui/event_reminder_ok"),
            ]),
        ),
        (
            "cafe",
            compose(&[
                (&cafe, "page/cafe"),
                (&cafe, "ui/cafe_income_entry"),
                (&cafe, "ui/return_home_icon"),
            ]),
        ),
    ];
    let home = &frames[0].1.0;

    let sealed_pack = json_entry(&cafe, ".pack.json");
    let sealed_pages = json_entry(&cafe, ".pages.json");
    emit(&format!(
        "sealed cafe package pack_schema={} pages_schema={}",
        sealed_pack["schema_version"], sealed_pages["schema_version"]
    ));

    // Authoring start path: a draft digest and a draft color probe observed on the home frame.
    let mut draft = sealed_pack.clone();
    draft["schema_version"] = json!("0.7");
    let mut draft_digest = digest_target(&"00".repeat(3 * 64));
    draft_digest["max_mean_milli"] = json!(0);
    push(&mut draft, draft_digest);
    push(
        &mut draft,
        color_target("state/home_cafe_tint", [0, 0, 0], None),
    );
    let draft_bundle = loaded(&rebuild(&cafe, &draft, &sealed_pages));
    let draft_evaluator = draft_bundle.loaded_bundle().evaluator().expect("evaluator");
    let observed = draft_evaluator
        .evaluate_target(home, "digest/home_cafe")
        .expect("draft digest");
    let observed_digest = observed.color_digest.as_ref().expect("digest evidence");
    let cells = observed_digest.observed_cells.clone();
    emit(&format!(
        "authoring draft {} observed_cells={cells}",
        summary(&observed)
    ));
    let observed_color = draft_evaluator
        .evaluate_target(home, "state/home_cafe_tint")
        .expect("draft color");
    let mean = observed_color.color.expect("color evidence").mean;
    emit(&format!("authoring draft {}", summary(&observed_color)));

    // The schema 0.7 pack: digest, composites and per-target color thresholds.
    let ocr = target(&json_entry(&readings, ".pack.json"), "ocr/stamina").clone();
    let mut pack = sealed_pack.clone();
    pack["schema_version"] = json!("0.7");
    push(&mut pack, digest_target(&cells));
    push(&mut pack, ocr.clone());
    push(
        &mut pack,
        color_target("state/home_cafe_tint", mean, Some(12.0)),
    );
    push(
        &mut pack,
        color_target("state/home_cafe_tint_default", mean, None),
    );
    push(
        &mut pack,
        composite("check/home", "all_of", &["digest/home_cafe", "page/home"]),
    );
    push(
        &mut pack,
        composite(
            "check/home_any",
            "any_of",
            &["page/home", "digest/home_cafe"],
        ),
    );
    push(
        &mut pack,
        composite(
            "check/home_text",
            "all_of",
            &["digest/home_cafe", "ocr/stamina"],
        ),
    );
    let (x, y, width, height) = DIGEST_REGION;
    for declared in pack["targets"].as_array_mut().expect("targets") {
        if declared["id"] == "ui/home_cafe_label" {
            declared["color_check"] = json!({
                "region": {"x": x, "y": y, "width": width, "height": height},
                "expected": mean,
                "max_distance": 12.0
            });
        }
    }
    let mut pages = sealed_pages.clone();
    for page in pages["pages"].as_array_mut().expect("pages") {
        if page["id"].as_str().is_some_and(|id| id.ends_with("/home")) {
            page["required"] = json!(["check/home", "ui/home_work_label"]);
            page["optional"] =
                json!(["digest/home_cafe", "check/home_any", "state/home_cafe_tint"]);
        }
    }
    for id in [
        "digest/home_cafe",
        "check/home",
        "check/home_any",
        "check/home_text",
        "state/home_cafe_tint",
        "ui/home_cafe_label",
    ] {
        emit(&format!("declared 0.7 {}", target(&pack, id)));
    }
    let zip = rebuild(&cafe, &pack, &pages);
    let prepared = admit(&zip);
    emit(&format!(
        "admission cafe package with 0.7 pack: {}",
        verdict(&prepared)
    ));
    let bundle07 = loaded(&zip);
    let evaluator = bundle07.loaded_bundle().evaluator().expect("evaluator");
    let detector = bundle07.loaded_bundle().detector().expect("detector");
    let sealed = loaded(&zip_entries(&cafe));
    let sealed_evaluator = sealed.loaded_bundle().evaluator().expect("evaluator");
    let sealed_detector = sealed.loaded_bundle().detector().expect("detector");

    for (name, (scene, canvas)) in &frames {
        for id in [
            "digest/home_cafe",
            "page/home",
            "check/home",
            "check/home_any",
            "state/home_cafe_tint",
            "state/home_cafe_tint_default",
            "ui/home_cafe_label",
        ] {
            let evaluation = evaluator
                .evaluate_target(scene, id)
                .unwrap_or_else(|error| panic!("{name} {id}: {error}"));
            emit(&format!("frame={name} {}", summary(&evaluation)));
        }
        match evaluator.evaluate_target(scene, "check/home_text") {
            Ok(evaluation) => emit(&format!("frame={name} {}", summary(&evaluation))),
            Err(error) => emit(&format!(
                "frame={name} check/home_text error code={:?} {error}",
                error.code()
            )),
        }
        let matched =
            |detector: &actingcommand_page_detector::PageDetector,
             evaluator: &actingcommand_recognition_pack::RecognitionEvaluator| {
                match detector.evaluate_all_outcomes(evaluator, scene) {
                    Ok(outcomes) => outcomes
                        .iter()
                        .map(|outcome| match &outcome.result {
                            Ok(page) => format!("{}={}", page.page_id, page.matched),
                            Err(error) => format!("{}=error {error}", outcome.page_id),
                        })
                        .collect::<Vec<_>>()
                        .join(","),
                    Err(error) => format!("batch error {error}"),
                }
            };
        emit(&format!(
            "frame={name} pages 0.7=[{}]",
            matched(detector, evaluator)
        ));
        emit(&format!(
            "frame={name} pages sealed 0.6=[{}]",
            matched(sealed_detector, sealed_evaluator)
        ));
        if let Ok(task) = &prepared {
            emit(&format!(
                "frame={name} kernel home preflight {}",
                kernel_home(task, canvas)
            ));
        }
    }
    let serialized = evaluator
        .evaluate_target(home, "check/home")
        .expect("check/home on home");
    emit(&format!(
        "frame=home check/home serialized={}",
        serde_json::to_string(&serialized).expect("serialize")
    ));

    // A second check with an OCR member where the page does not gate post-admission OCR.
    let mut text_pages = sealed_pages.clone();
    for page in text_pages["pages"].as_array_mut().expect("pages") {
        if page["id"].as_str().is_some_and(|id| id.ends_with("/home")) {
            page["required"] = json!(["check/home_text", "ui/home_work_label"]);
        }
    }
    emit(&format!(
        "admission cafe package, home = check(digest and ocr): {}",
        verdict(&admit(&rebuild(&cafe, &pack, &text_pages)))
    ));

    // The reading package gates post-admission OCR on its home page.
    let mut readings_pack = json_entry(&readings, ".pack.json");
    readings_pack["schema_version"] = json!("0.7");
    push(&mut readings_pack, digest_target(&cells));
    let mut label = ocr.clone();
    label["id"] = json!("ocr/stamina_label");
    push(&mut readings_pack, label);
    push(
        &mut readings_pack,
        composite("check/home", "all_of", &["digest/home_cafe", "page/home"]),
    );
    push(
        &mut readings_pack,
        composite(
            "check/home_label",
            "all_of",
            &["digest/home_cafe", "ocr/stamina_label"],
        ),
    );
    push(
        &mut readings_pack,
        composite(
            "check/home_field",
            "all_of",
            &["digest/home_cafe", "ocr/stamina"],
        ),
    );
    let readings_pages = json_entry(&readings, ".pages.json");
    emit(&format!(
        "reading package sealed admission: {}",
        verdict(&admit(&zip_entries(&readings)))
    ));
    for (case, required) in [
        ("gate home = check(digest and template)", "check/home"),
        ("gate home = check(digest and ocr)", "check/home_label"),
        (
            "gate home = check(digest and the ocr field target)",
            "check/home_field",
        ),
    ] {
        let mut gate_pages = readings_pages.clone();
        gate_pages["pages"][0]["required"] = json!([required]);
        emit(&format!(
            "reading package {case}: {}",
            verdict(&admit(&rebuild(&readings, &readings_pack, &gate_pages)))
        ));
    }

    // Ordinary authoring mistakes are refused at admission.
    let mut regrid = pack.clone();
    for declared in regrid["targets"].as_array_mut().expect("targets") {
        if declared["id"] == "digest/home_cafe" {
            declared["columns"] = json!(6);
        }
    }
    let mut typo = pack.clone();
    for declared in typo["targets"].as_array_mut().expect("targets") {
        if declared["id"] == "check/home" {
            declared["members"] = json!(["digest/home_cafee", "page/home"]);
        }
    }
    let mut stale = pack.clone();
    stale["schema_version"] = json!("0.6");
    for (case, mistaken) in [
        ("grid changed without recomputing cells", regrid),
        ("member id misspelled", typo),
        ("0.7 constructs in a 0.6 pack", stale),
    ] {
        emit(&format!(
            "mistake {case}: {}",
            verdict(&admit(&rebuild(&cafe, &mistaken, &pages)))
        ));
    }
}
