// SPDX-License-Identifier: AGPL-3.0-only

//! One-off evidence for Workflow #308 G1, to be reverted.
//!
//! 1. The golden projection of `contracts/candidate-projection.md` is produced by
//!    `project_candidates` from the hand-written neutral schema 0.7 fragment of the contract and
//!    a frame composed of one bundle image at the declared rectangle. Pixels of that image are
//!    shifted, in a fixed order, until the mark target scores in the golden's 0.991 band.
//! 2. A copy of the published schedule package gains a schema 0.7 pack with a `fixed_slots`
//!    layout over three slots of its course list page, is admitted through the production
//!    loader, and is projected on frames composed of the bundle's own images at their declared
//!    rectangles, twice each.
//! 3. Realistic authoring mistakes are refused with the pointer of the offending field.

use actingcommand_contract::candidate_projection::CandidateProjection;
use actingcommand_execution_kernel::{
    ExternalExpectedSha256, ExternallyVerifiedBundle, PreparedContainedTask,
    PreparedPageObservation,
};
use actingcommand_pack_containment::Sha256Hash;
use actingcommand_recognition::Scene;
use actingcommand_recognition_pack::{
    AssetResolver, RecognitionEvaluator, RecognitionPackError, RecognitionPackResult,
    load_pack_from_json_str,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::process::Command;
use std::sync::Arc;

type Entries = BTreeMap<String, Vec<u8>>;

const ARCHIVE_URL: &str =
    "https://github.com/HS7097/ActingCommand/archive/536f048a3cac26ddbb0391895391f98967704cb0.zip";
const BUNDLE_SHA256: &str = "df524eac6290c4c0b435f3872a89e12dbe4f49e37555fa102bf96dd71b696af8";
const WIDTH: usize = 1280;
const HEIGHT: usize = 720;

/// The neutral schema 0.7 fragment of the contract example: one slot of `list_page`.
const GOLDEN_PACK: &str = r#"{
  "schema_version": "0.7",
  "coordinate_space": {"width": 1280, "height": 720},
  "defaults": {"template_threshold": 0.97, "color_max_distance": 20, "match_metric": "ccoeff_normed"},
  "targets": [
    {"type": "template", "id": "ui/slot_0_mark", "template_path": "assets/slot_mark.png",
     "region": {"x": 214, "y": 204, "width": 176, "height": 27}, "threshold": 0.97},
    {"type": "color", "id": "state/slot_0_open",
     "region": {"x": 224, "y": 250, "width": 20, "height": 10}, "expected": [0, 0, 0]}
  ],
  "candidate_layouts": [
    {"id": "layout/main_slots", "page_id": "list_page", "kind": "fixed_slots",
     "features": [
       {"name": "open", "value": "passed"},
       {"name": "marked", "value": "passed"},
       {"name": "mark_score", "value": "measure_milli"}],
     "slots": [
       {"rect":  {"x": 214, "y": 204, "width": 176, "height": 90},
        "click": {"x": 224, "y": 214, "width": 156, "height": 60},
        "targets": {"open": "state/slot_0_open", "marked": "ui/slot_0_mark", "mark_score": "ui/slot_0_mark"}}]}]
}"#;

/// The golden projection, verbatim from `contracts/candidate-projection.md`.
const GOLDEN_PROJECTION: &str = r#"{"schema_version": "actingcommand.candidate-projection.v1",
 "page_id": "list_page", "layout_id": "layout/main_slots", "layout_kind": "fixed_slots",
 "frame": {"width": 1280, "height": 720},
 "candidates": [
   {"id": "layout/main_slots#00", "instance_index": 0, "actionable": true,
    "rect": {"x": 214, "y": 204, "width": 176, "height": 90},
    "click": {"x": 224, "y": 214, "width": 156, "height": 60},
    "features": {"mark_score": {"type": "integer", "value": 991, "confidence": 991},
                 "marked": {"type": "boolean", "value": true, "confidence": 991},
                 "open": {"type": "boolean", "value": true, "confidence": null}}}],
 "candidate_set_sha256": "e45f4db1262ff314e6ffd40c588bfc02bbf4be76d9eb626434c7158488719e37"}"#;

fn emit(line: &str) {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "G1-PROJECTION {}", line.replace('\n', " | "))
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

/// The package with its derived pack replaced and the manifest hash renewed.
fn rebuild(entries: &Entries, pack: &Value) -> Vec<u8> {
    let mut package = entries.clone();
    let pack_path = entry_path(entries, ".pack.json");
    package.insert(pack_path.clone(), serde_json::to_vec(pack).expect("pack"));
    let mut manifest = json_entry(entries, "resources/manifest.json");
    for file in manifest["files"].as_array_mut().expect("manifest files") {
        let path = format!("resources/{}", file["path"].as_str().expect("file path"));
        if path == pack_path {
            file["sha256"] = json!(format!("sha256:{}", Sha256Hash::digest(&package[&path])));
        }
    }
    package.insert(
        "resources/manifest.json".to_owned(),
        serde_json::to_vec(&manifest).expect("manifest"),
    );
    zip_entries(&package)
}

fn draw(canvas: &mut [u8], crop: &Scene, x: usize, y: usize) {
    let crop_width = crop.width() as usize;
    assert!(x + crop_width <= WIDTH && y + crop.height() as usize <= HEIGHT);
    for row in 0..crop.height() as usize {
        let source = &crop.rgb8_pixels()[row * crop_width * 3..(row + 1) * crop_width * 3];
        let start = ((y + row) * WIDTH + x) * 3;
        canvas[start..start + crop_width * 3].copy_from_slice(source);
    }
}

fn crop_of(entries: &Entries, pack: &Value, id: &str) -> Scene {
    let path = target(pack, id)["template_path"]
        .as_str()
        .expect("template_path");
    Scene::from_png(&entries[&format!("resources/{path}")])
        .unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn declared_origin(pack: &Value, id: &str) -> (usize, usize) {
    let region = &target(pack, id)["region"];
    (
        region["x"].as_u64().expect("x") as usize,
        region["y"].as_u64().expect("y") as usize,
    )
}

fn scene(canvas: &[u8]) -> Scene {
    Scene::from_rgb8(WIDTH as u32, HEIGHT as u32, canvas).unwrap_or_else(|error| panic!("{error}"))
}

fn admit(zip: &[u8]) -> String {
    match PreparedContainedTask::load("one-off.instance", zip, expected(zip)) {
        Ok(_) => "ok".to_owned(),
        Err(error) => format!("{error:?}"),
    }
}

fn observe(zip: &[u8]) -> String {
    match PreparedPageObservation::load("one-off.instance", zip, expected(zip), &[], None) {
        Ok(_) => "ok".to_owned(),
        Err(error) => format!(
            "code={} stage={} cause={}",
            error.code(),
            error.stage(),
            error.cause()
        ),
    }
}

fn summary(projection: &CandidateProjection) -> String {
    projection
        .candidates()
        .iter()
        .map(|candidate| {
            let features = candidate
                .features
                .iter()
                .map(|(name, feature)| format!("{name}={}", json!(feature)))
                .collect::<Vec<_>>()
                .join(" ");
            format!(
                "{} actionable={} rect={} click={} [{features}]",
                candidate.id,
                candidate.actionable,
                json!(candidate.rect),
                json!(candidate.click)
            )
        })
        .collect::<Vec<_>>()
        .join(" ; ")
}

#[derive(Debug)]
struct OneAsset {
    path: String,
    bytes: Vec<u8>,
}

impl AssetResolver for OneAsset {
    fn read_asset(&self, path: &str) -> RecognitionPackResult<Vec<u8>> {
        if path == self.path {
            Ok(self.bytes.clone())
        } else {
            Err(RecognitionPackError::fatal(format!("no asset {path}")))
        }
    }

    fn contains_asset(&self, path: &str) -> bool {
        path == self.path
    }
}

/// The mark image at the declared rectangle with its first `shifted` pixels, in a fixed
/// pseudo-random order, moved by a fixed pseudo-random amount.
fn golden_canvas(mark: &Scene, shifted: usize) -> Vec<u8> {
    let mut canvas = vec![0_u8; WIDTH * HEIGHT * 3];
    draw(&mut canvas, mark, 214, 204);
    let (mark_width, mark_height) = (mark.width() as usize, mark.height() as usize);
    let count = mark_width * mark_height;
    for step in 0..shifted {
        let position = (step * 1009) % count;
        let hash = (position as u64)
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407)
            >> 33;
        let delta = (hash % 81) as i16 - 40;
        let (x, y) = (214 + position % mark_width, 204 + position / mark_width);
        let start = (y * WIDTH + x) * 3;
        for channel in &mut canvas[start..start + 3] {
            *channel = (i16::from(*channel) + delta).clamp(0, 255) as u8;
        }
    }
    canvas
}

fn golden(mark_png: &[u8]) {
    let pack = load_pack_from_json_str(GOLDEN_PACK).unwrap_or_else(|error| panic!("{error}"));
    let evaluator = RecognitionEvaluator::with_asset_resolver(
        pack,
        Arc::new(OneAsset {
            path: "assets/slot_mark.png".to_owned(),
            bytes: mark_png.to_vec(),
        }),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let mark = Scene::from_png(mark_png).unwrap_or_else(|error| panic!("{error}"));
    let count = mark.width() as usize * mark.height() as usize;
    let milli = |shifted: usize| {
        let evaluation = evaluator
            .evaluate_target(&scene(&golden_canvas(&mark, shifted)), "ui/slot_0_mark")
            .unwrap_or_else(|error| panic!("{error}"));
        let score = evaluation.template.expect("template evidence").score;
        (score, (f64::from(score) * 1000.0).floor() as i64)
    };
    emit(&format!(
        "golden mark {}x{} unshifted score={:?}",
        mark.width(),
        mark.height(),
        milli(0)
    ));
    // Coarse steps until the score reaches the band, then single pixels back from an overshoot.
    let mut found = None;
    let mut coarse = 0;
    while coarse <= count && found.is_none() {
        let (score, value) = milli(coarse);
        if value == 991 {
            found = Some((coarse, score));
        } else if value < 991 {
            emit(&format!(
                "golden coarse overshoot at shifted={coarse} score={score:?} milli={value}"
            ));
            found = (coarse.saturating_sub(49)..coarse)
                .map(|shifted| (shifted, milli(shifted)))
                .find(|(_, (_, value))| *value == 991)
                .map(|(shifted, (score, _))| (shifted, score));
            break;
        }
        coarse += 50;
    }
    let Some((shifted, score)) = found else {
        emit("golden band 991 not reachable");
        return;
    };
    emit(&format!(
        "golden frame shifted_pixels={shifted} score={score:?}"
    ));
    let projection = |shifted: usize| {
        evaluator
            .scene_context(&scene(&golden_canvas(&mark, shifted)))
            .project_candidates("layout/main_slots")
            .unwrap_or_else(|error| panic!("{error}"))
    };
    let produced = projection(shifted);
    let again = projection(shifted);
    let reference: CandidateProjection =
        serde_json::from_str(GOLDEN_PROJECTION).unwrap_or_else(|error| panic!("{error}"));
    reference
        .validate()
        .unwrap_or_else(|error| panic!("golden: {error}"));
    emit(&format!(
        "golden produced={}",
        serde_json::to_string(&produced).expect("serialize")
    ));
    emit(&format!(
        "golden produced_sha256={} golden_sha256={} equal_to_golden={} second_run_identical={}",
        produced.candidate_set_sha256(),
        reference.candidate_set_sha256(),
        produced == reference,
        serde_json::to_vec(&produced).expect("serialize")
            == serde_json::to_vec(&again).expect("serialize")
    ));
}

#[test]
fn one_off_g1_candidate_projection_on_bundle_frames() {
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
    let schedule = package(&bundle, ".schedule_daily.zip");
    let sealed_pack = json_entry(&schedule, ".pack.json");
    let pages = json_entry(&schedule, ".pages.json");
    let course_path = target(&sealed_pack, "ui/schedule_course_tower")["template_path"]
        .as_str()
        .expect("template_path")
        .to_owned();
    let course_png = schedule[&format!("resources/{course_path}")].clone();

    // 1. The contract golden.
    golden(&course_png);

    // 2. A fixed_slots layout over three slots of the course list page.
    let page_id = pages["pages"]
        .as_array()
        .expect("pages")
        .iter()
        .filter_map(|page| page["id"].as_str())
        .find(|id| id.ends_with("/schedule_all"))
        .expect("course list page")
        .to_owned();
    let course = Scene::from_png(&course_png).unwrap_or_else(|error| panic!("{error}"));
    let (probe_x, probe_y, probe_width, probe_height) = (8_usize, 8_usize, 16_usize, 10_usize);
    let mut sum = [0_u64; 3];
    for row in probe_y..probe_y + probe_height {
        for column in probe_x..probe_x + probe_width {
            let start = (row * course.width() as usize + column) * 3;
            for (channel, total) in sum.iter_mut().enumerate() {
                *total += u64::from(course.rgb8_pixels()[start + channel]);
            }
        }
    }
    let area = (probe_width * probe_height) as u64;
    let open_color = sum.map(|total| total / area);
    emit(&format!(
        "course image {course_path} {}x{} open probe mean={open_color:?}",
        course.width(),
        course.height()
    ));
    let slot_x = |slot: usize| 214 + 290 * slot;
    let mut pack = sealed_pack.clone();
    pack["schema_version"] = json!("0.7");
    for slot in 0..3 {
        let targets = pack["targets"].as_array_mut().expect("targets");
        targets.push(json!({
            "type": "template",
            "id": format!("ui/slot_{slot}_course"),
            "template_path": course_path,
            "region": {"x": slot_x(slot), "y": 204, "width": course.width(), "height": course.height()},
            "threshold": 0.97
        }));
        targets.push(json!({
            "type": "color",
            "id": format!("state/slot_{slot}_open"),
            "region": {"x": slot_x(slot) + probe_x, "y": 204 + probe_y, "width": probe_width, "height": probe_height},
            "expected": open_color
        }));
    }
    let slots = (0..3)
        .map(|slot| {
            json!({
                "rect": {"x": slot_x(slot), "y": 204, "width": 176, "height": 90},
                "click": {"x": slot_x(slot) + 10, "y": 214, "width": 156, "height": 60},
                "targets": {
                    "open": format!("state/slot_{slot}_open"),
                    "marked": format!("ui/slot_{slot}_course"),
                    "mark_score": format!("ui/slot_{slot}_course"),
                    "open_distance": format!("state/slot_{slot}_open")
                }
            })
        })
        .collect::<Vec<_>>();
    pack["candidate_layouts"] = json!([{
        "id": "layout/course_slots",
        "page_id": page_id,
        "kind": "fixed_slots",
        "features": [
            {"name": "open", "value": "passed"},
            {"name": "marked", "value": "passed"},
            {"name": "mark_score", "value": "measure_milli"},
            {"name": "open_distance", "value": "measure_milli"}
        ],
        "slots": slots
    }]);
    emit(&format!(
        "declared candidate_layouts={}",
        serde_json::to_string(&pack["candidate_layouts"]).expect("layouts")
    ));
    let zip = rebuild(&schedule, &pack);
    emit(&format!("admission with layout: {}", admit(&zip)));
    emit(&format!(
        "observation preparation with layout: {}",
        observe(&zip)
    ));
    let loaded = ExternallyVerifiedBundle::load("one-off.instance", &zip, expected(&zip))
        .unwrap_or_else(|error| panic!("load: {error}"));
    let evaluator = loaded.loaded_bundle().evaluator().expect("evaluator");
    let detector = loaded.loaded_bundle().detector().expect("detector");

    let mut page_canvas = vec![0_u8; WIDTH * HEIGHT * 3];
    for id in [
        "page/schedule_all",
        "ui/schedule_all_close",
        "ui/schedule_course_tower",
    ] {
        let (x, y) = declared_origin(&sealed_pack, id);
        draw(
            &mut page_canvas,
            &crop_of(&schedule, &sealed_pack, id),
            x,
            y,
        );
    }
    let mut two_canvas = page_canvas.clone();
    draw(&mut two_canvas, &course, slot_x(2), 204);
    let mut hashes = Vec::new();
    for (name, canvas) in [
        ("course_in_slot_0", &page_canvas),
        ("course_in_slots_0_and_2", &two_canvas),
    ] {
        let frame = scene(canvas);
        let matched = match detector.evaluate_all_outcomes(evaluator, &frame) {
            Ok(outcomes) => outcomes
                .iter()
                .filter_map(|outcome| match &outcome.result {
                    Ok(page) if page.matched => Some(page.page_id.clone()),
                    Ok(_) => None,
                    Err(error) => Some(format!("{}:error {error}", outcome.page_id)),
                })
                .collect::<Vec<_>>()
                .join(","),
            Err(error) => format!("batch error {error}"),
        };
        let first = evaluator
            .scene_context(&frame)
            .project_candidates("layout/course_slots")
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let second = evaluator
            .scene_context(&scene(canvas))
            .project_candidates("layout/course_slots")
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let bytes = serde_json::to_vec(&first).expect("serialize");
        emit(&format!(
            "frame={name} matched=[{matched}] candidates={} hash={} second_run_identical={}",
            first.candidates().len(),
            first.candidate_set_sha256(),
            bytes == serde_json::to_vec(&second).expect("serialize")
        ));
        emit(&format!("frame={name} {}", summary(&first)));
        emit(&format!(
            "frame={name} projection={}",
            String::from_utf8(bytes).expect("utf-8")
        ));
        hashes.push(first.candidate_set_sha256().to_owned());
    }
    emit(&format!(
        "hashes differ between frames: {}",
        hashes[0] != hashes[1]
    ));

    // 3. Realistic authoring mistakes.
    let mut copied_click = pack.clone();
    copied_click["candidate_layouts"][0]["slots"][2]["click"] =
        json!({"x": 1206, "y": 321, "width": 234, "height": 90});
    let mut misspelled_target = pack.clone();
    misspelled_target["candidate_layouts"][0]["slots"][1]["targets"]["open"] =
        json!("state/slot_1_opne");
    let mut misspelled_page = pack.clone();
    misspelled_page["candidate_layouts"][0]["page_id"] =
        json!(page_id.trim_end_matches('l').to_owned());
    let mut old_schema = pack.clone();
    old_schema["schema_version"] = json!("0.6");
    for (case, mistaken) in [
        (
            "slot 2 click copied from a 1920x1080 capture",
            &copied_click,
        ),
        ("slot 1 open target misspelled", &misspelled_target),
        ("layout page misspelled", &misspelled_page),
        ("layouts in a pack left at schema 0.6", &old_schema),
    ] {
        let zip = rebuild(&schedule, mistaken);
        emit(&format!("mistake {case}: admission {}", admit(&zip)));
        emit(&format!(
            "mistake {case}: observation preparation {}",
            observe(&zip)
        ));
    }
}
