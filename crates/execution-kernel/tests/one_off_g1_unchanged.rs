// SPDX-License-Identifier: AGPL-3.0-only

//! One-off evidence for Workflow #308 G1, to be reverted. Every sealed package of the published
//! bundle (recognition pack schema 0.6) and a copy of the schedule package whose pack is schema
//! 0.7 without candidate layouts are admitted through the production loader, and their pages are
//! judged on frames composed of the bundle's own images at their declared rectangles. The same
//! file runs before and after the G1 change; equal output shows that packs without layouts
//! load and judge exactly as before.

use actingcommand_execution_kernel::{
    ExternalExpectedSha256, ExternallyVerifiedBundle, PreparedContainedTask,
};
use actingcommand_pack_containment::Sha256Hash;
use actingcommand_recognition::Scene;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::process::Command;

type Entries = BTreeMap<String, Vec<u8>>;

const ARCHIVE_URL: &str =
    "https://github.com/HS7097/ActingCommand/archive/536f048a3cac26ddbb0391895391f98967704cb0.zip";
const BUNDLE_SHA256: &str = "df524eac6290c4c0b435f3872a89e12dbe4f49e37555fa102bf96dd71b696af8";

fn emit(line: &str) {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "G1-UNCHANGED {}", line.replace('\n', " | "))
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

/// A frame of the package's coordinate space showing the bundle's own image of every
/// template the page requires, each at the rectangle the package declares for it; every
/// other pixel is black. The bundle carries these crops, not whole screenshots.
fn page_frame(entries: &Entries, pack: &Value, page: &Value) -> (Scene, Vec<String>) {
    let width = pack["coordinate_space"]["width"].as_u64().expect("width") as usize;
    let height = pack["coordinate_space"]["height"].as_u64().expect("height") as usize;
    let mut canvas = vec![0_u8; width * height * 3];
    let targets = pack["targets"].as_array().expect("targets");
    let mut drawn = Vec::new();
    let ids = page["required"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(
            page["any_of"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|group| group.as_array().into_iter().flatten()),
        )
        .filter_map(Value::as_str);
    for id in ids {
        let Some(target) = targets
            .iter()
            .find(|target| target["id"] == id && target["type"] == "template")
        else {
            continue;
        };
        let (Some(path), Some(region)) = (
            target["template_path"].as_str(),
            target["region"].as_object(),
        ) else {
            continue;
        };
        let bytes = entries
            .get(&format!("resources/{path}"))
            .unwrap_or_else(|| panic!("missing template {path}"));
        let crop = Scene::from_png(bytes).unwrap_or_else(|error| panic!("{path}: {error}"));
        let x = region["x"].as_u64().expect("x") as usize;
        let y = region["y"].as_u64().expect("y") as usize;
        let (crop_width, crop_height) = (crop.width() as usize, crop.height() as usize);
        assert!(
            x + crop_width <= width && y + crop_height <= height,
            "{path}"
        );
        for row in 0..crop_height {
            let source = &crop.rgb8_pixels()[row * crop_width * 3..(row + 1) * crop_width * 3];
            let start = ((y + row) * width + x) * 3;
            canvas[start..start + crop_width * 3].copy_from_slice(source);
        }
        drawn.push(id.to_owned());
    }
    let scene = Scene::from_rgb8(width as u32, height as u32, &canvas)
        .unwrap_or_else(|error| panic!("{error}"));
    (scene, drawn)
}

/// Admits the package and judges every page of it on its composed frame; returns the digest of
/// the serialized page outcomes.
fn judge(label: &str, zip: &[u8]) -> Vec<u8> {
    let admitted = match PreparedContainedTask::load("one-off.instance", zip, expected(zip)) {
        Ok(_) => "ok".to_owned(),
        Err(error) => format!("error {error:?}"),
    };
    let bundle = ExternallyVerifiedBundle::load("one-off.instance", zip, expected(zip))
        .unwrap_or_else(|error| panic!("{label}: {error}"));
    let loaded = bundle.loaded_bundle();
    let evaluator = loaded.evaluator().expect("evaluator");
    let detector = loaded.detector().expect("detector");
    let entries = unzip(zip);
    let pack = json_entry(&entries, ".pack.json");
    let pages = json_entry(&entries, ".pages.json");
    emit(&format!(
        "pack={label} pack_schema={} pages_schema={} targets={} admitted={admitted}",
        pack["schema_version"],
        pages["schema_version"],
        pack["targets"].as_array().map_or(0, Vec::len)
    ));
    let mut digest = Sha256::new();
    for page in pages["pages"].as_array().expect("pages") {
        let (scene, drawn) = page_frame(&entries, &pack, page);
        let outcomes = detector.evaluate_all_outcomes(evaluator, &scene);
        let serialized = match &outcomes {
            Ok(outcomes) => serde_json::to_vec(outcomes).expect("serialize outcomes"),
            Err(error) => format!("batch error {error}").into_bytes(),
        };
        digest.update(&serialized);
        let matched = match &outcomes {
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
        emit(&format!(
            "  frame_for={} drawn=[{}] matched=[{matched}] outcome_sha256={}",
            page["id"].as_str().unwrap_or("?"),
            drawn.join(","),
            hex(&Sha256::digest(&serialized))
        ));
    }
    let digest = digest.finalize().to_vec();
    emit(&format!("pack={label} judgments_sha256={}", hex(&digest)));
    digest
}

#[test]
fn one_off_g1_packs_without_layouts_admit_and_judge_as_before() {
    let archive = unzip(&download_archive());
    let (bundle_name, bundle_bytes) = archive
        .iter()
        .find(|(path, _)| path.contains("/bundles/") && path.ends_with(".zip"))
        .unwrap_or_else(|| panic!("no bundle in the archive"));
    let bundle_sha256 = Sha256Hash::digest(bundle_bytes).to_string();
    assert_eq!(bundle_sha256, BUNDLE_SHA256, "{bundle_name}");
    let bundle = unzip(bundle_bytes);
    let index: Value = serde_json::from_slice(
        bundle
            .get("bundle.json")
            .unwrap_or_else(|| panic!("bundle.json missing")),
    )
    .unwrap_or_else(|error| panic!("bundle.json: {error}"));
    let packs = index["packs"].as_array().expect("bundle.json packs");
    emit(&format!("bundle={bundle_sha256} packs={}", packs.len()));

    let mut sealed = Sha256::new();
    let mut schedule = None;
    for pack in packs {
        let path = pack["path"].as_str().expect("pack path");
        let bytes = bundle.get(path).unwrap_or_else(|| panic!("missing {path}"));
        assert_eq!(
            Sha256Hash::digest(bytes).to_string(),
            pack["sha256"].as_str().expect("sha256"),
            "{path}"
        );
        sealed.update(judge(path, bytes));
        if path.ends_with(".schedule_daily.zip") {
            schedule = Some(unzip(bytes));
        }
    }
    emit(&format!(
        "sealed_packages judgments_sha256={}",
        hex(&sealed.finalize())
    ));

    // The schedule package with a schema 0.7 pack: one added composite, no candidate layouts.
    let schedule = schedule.expect("schedule package");
    let mut pack = json_entry(&schedule, ".pack.json");
    pack["schema_version"] = json!("0.7");
    pack["targets"]
        .as_array_mut()
        .expect("targets")
        .push(json!({
            "type": "composite",
            "id": "check/schedule_all_ready",
            "mode": "all_of",
            "members": ["page/schedule_all", "ui/schedule_all_close"]
        }));
    let zip = rebuild(&schedule, &pack);
    judge("schedule copy with pack 0.7", &zip);
    let bundle07 = ExternallyVerifiedBundle::load("one-off.instance", &zip, expected(&zip))
        .unwrap_or_else(|error| panic!("0.7 copy: {error}"));
    let evaluator = bundle07.loaded_bundle().evaluator().expect("evaluator");
    let pages = json_entry(&schedule, ".pages.json");
    let page = pages["pages"]
        .as_array()
        .expect("pages")
        .iter()
        .find(|page| {
            page["id"]
                .as_str()
                .is_some_and(|id| id.ends_with("/schedule_all"))
        })
        .expect("schedule_all page");
    let (scene, _) = page_frame(&schedule, &pack, page);
    let composite = evaluator
        .evaluate_target(&scene, "check/schedule_all_ready")
        .unwrap_or_else(|error| panic!("composite: {error}"));
    let serialized = serde_json::to_vec(&composite).expect("serialize composite");
    emit(&format!(
        "schedule copy with pack 0.7 frame_for={} check/schedule_all_ready passed={} evaluation_sha256={}",
        page["id"].as_str().unwrap_or("?"),
        composite.passed,
        hex(&Sha256::digest(&serialized))
    ));
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
