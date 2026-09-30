// SPDX-License-Identifier: AGPL-3.0-only

//! One-off evidence for the #308 D1b review finding, to be reverted. An OCR target that the
//! package's companion projection declaration marks personal is judged on the home frame
//! (composed of the bundle's own images at their declared rectangles) through the online
//! observation owner, once inside a page composite and once referenced by the page directly;
//! the sealed package is observed as the unchanged baseline. The same file runs before and
//! after the fix. CI has no OCR engine, so a fixed provider returns a marker text for every
//! OCR request; the lines report where that marker appears.

use actingcommand_contract::page_projection::{FrameIdentity, FrameKind};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_execution_kernel::{ExternalExpectedSha256, PreparedPageObservation};
use actingcommand_pack_containment::Sha256Hash;
use actingcommand_recognition::color_digest::{ColorDigest, ColorDigestGrid};
use actingcommand_recognition::{Rect, Scene};
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, VisionProvider, VisionProviderError, VisionProviderErrorCode,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::process::Command;
use std::sync::Arc;

type Entries = BTreeMap<String, Vec<u8>>;

const ARCHIVE_URL: &str =
    "https://github.com/HS7097/ActingCommand/archive/536f048a3cac26ddbb0391895391f98967704cb0.zip";
const BUNDLE_SHA256: &str = "df524eac6290c4c0b435f3872a89e12dbe4f49e37555fa102bf96dd71b696af8";
const MARKER: &str = "STUB-OCR-4821";

fn emit(line: &str) {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "D1B-PRIVACY {}", line.replace('\n', " | "))
        .unwrap_or_else(|error| panic!("write stdout: {error}"));
    stdout
        .flush()
        .unwrap_or_else(|error| panic!("flush stdout: {error}"));
}

#[derive(Debug)]
struct MarkerOcr;

impl VisionProvider for MarkerOcr {
    fn require_ocr_model(&self, _: &str, _: &str) -> Result<(), VisionProviderError> {
        Ok(())
    }

    fn require_nn_model(&self, _: &str, _: &str) -> Result<(), VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN in this one-off",
        ))
    }

    fn read_text(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: MARKER.to_owned(),
            blocks: vec![OcrProviderTextBlock {
                text: MARKER.to_owned(),
                rect: request.region,
                confidence: Some(0.99),
            }],
            confidence: Some(0.99),
        })
    }

    fn classify(&self, _: NnProviderRequest<'_>) -> Result<NnProviderResult, VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN in this one-off",
        ))
    }
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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

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

/// The home frame: the package's own home label images at their declared rectangles.
fn home_frame(entries: &Entries) -> (Scene, Vec<u8>) {
    let (width, height) = (1280_usize, 720_usize);
    let mut canvas = vec![0_u8; width * height * 3];
    let pack = json_entry(entries, ".pack.json");
    for id in ["page/home", "ui/home_work_label"] {
        let declared = target(&pack, id);
        let path = declared["template_path"].as_str().expect("template_path");
        let crop = Scene::from_png(&entries[&format!("resources/{path}")])
            .unwrap_or_else(|error| panic!("{path}: {error}"));
        let x = declared["region"]["x"].as_u64().expect("x") as usize;
        let y = declared["region"]["y"].as_u64().expect("y") as usize;
        let crop_width = crop.width() as usize;
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

/// The package with its pack, page set and optional companion projection declaration
/// replaced, and the manifest hashes renewed.
fn rebuild(entries: &Entries, pack: &Value, pages: &Value, projection: Option<&Value>) -> Vec<u8> {
    let mut package = entries.clone();
    let pack_path = entry_path(entries, ".pack.json");
    let pages_path = entry_path(entries, ".pages.json");
    let projection_path =
        entry_path(entries, ".navigation.json").replace(".navigation.json", ".projection.json");
    package.insert(pack_path.clone(), serde_json::to_vec(pack).expect("pack"));
    package.insert(
        pages_path.clone(),
        serde_json::to_vec(pages).expect("pages"),
    );
    let mut manifest = json_entry(entries, "resources/manifest.json");
    if let Some(projection) = projection {
        package.insert(
            projection_path.clone(),
            serde_json::to_vec(projection).expect("projection"),
        );
        manifest["files"]
            .as_array_mut()
            .expect("manifest files")
            .push(json!({
                "path": projection_path.strip_prefix("resources/").expect("resource path"),
                "sha256": ""
            }));
    }
    for file in manifest["files"].as_array_mut().expect("manifest files") {
        let path = format!("resources/{}", file["path"].as_str().expect("file path"));
        if path == pack_path || path == pages_path || path == projection_path {
            file["sha256"] = json!(format!("sha256:{}", Sha256Hash::digest(&package[&path])));
        }
    }
    package.insert(
        "resources/manifest.json".to_owned(),
        serde_json::to_vec(&manifest).expect("manifest"),
    );
    zip_entries(&package)
}

/// Where the marker text appears, and the projection's own entries for the OCR target and
/// its composite.
fn observe(case: &str, zip: &[u8], png: &[u8], frame_sha256: &str) {
    let expected = ExternalExpectedSha256::parse_hex(&Sha256Hash::digest(zip).to_string())
        .unwrap_or_else(|error| panic!("{error:?}"));
    let prepared = PreparedPageObservation::load(
        "one-off.instance",
        zip,
        expected,
        &[],
        Some(Arc::new(MarkerOcr)),
    )
    .unwrap_or_else(|error| panic!("{case} load: {error:?} {}", error.cause()));
    let observed = prepared
        .evaluate(
            png,
            FrameIdentity {
                kind: FrameKind::Rgb8,
                sha256: frame_sha256.to_owned(),
                width: 1280,
                height: 720,
            },
        )
        .unwrap_or_else(|error| panic!("{case} evaluate: {error:?} {}", error.cause()));
    let projection = serde_json::to_string(&observed.projection).expect("projection");
    let facts = serde_json::to_string(&observed.facts).expect("facts");
    let private_facts = serde_json::to_string(&observed.private_facts).expect("private facts");
    let fields = observed
        .projection
        .fields
        .iter()
        .filter(|field| {
            field["target_id"]
                .as_str()
                .is_some_and(|id| id == "ocr/stamina" || id.starts_with("check/"))
        })
        .map(|field| {
            format!(
                "{}(redacted={} privacy={} value_present={})",
                field["target_id"].as_str().unwrap_or("?"),
                field["redacted"],
                field["privacy"],
                !field["value"].is_null()
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    emit(&format!(
        "case={case} page={} state={} marker_in_public_projection={} marker_in_public_facts={} marker_in_private_facts={} fields=[{fields}] projection_sha256={} facts_sha256={}",
        observed.projection.page,
        observed.projection.state,
        projection.matches(MARKER).count(),
        facts.matches(MARKER).count(),
        private_facts.matches(MARKER).count(),
        hex(&Sha256::digest(projection.as_bytes())),
        hex(&Sha256::digest(facts.as_bytes()))
    ));
}

#[test]
fn one_off_d1b_composite_member_privacy_in_observation() {
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
    let readings = package(&bundle, ".home_readings.zip");
    let (scene, canvas) = home_frame(&cafe);
    let png = Frame::from_pixels(
        1280,
        720,
        canvas.clone(),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .unwrap_or_else(|error| panic!("frame: {error}"))
    .encode_png_fast()
    .unwrap_or_else(|error| panic!("png: {error}"));
    let frame_sha256 = hex(&Sha256::digest(scene.rgb8_pixels()));
    let cells = ColorDigest::compute(
        &scene,
        Rect {
            x: 77,
            y: 680,
            width: 49,
            height: 18,
        },
        ColorDigestGrid::new(8, 8).expect("grid"),
    )
    .expect("digest")
    .to_hex();

    let ocr = target(&json_entry(&readings, ".pack.json"), "ocr/stamina").clone();
    let personal = json!({
        "schema_version": "actingcommand.page-projection-metadata.v1",
        "actions": [],
        "targets": [{"target_id": "ocr/stamina", "privacy": "personal", "source": "one-off/d1b"}],
        "fields": [],
        "pages": []
    });
    let sealed_pack = json_entry(&cafe, ".pack.json");
    let sealed_pages = json_entry(&cafe, ".pages.json");
    let home_pages = |required: Value| {
        let mut pages = sealed_pages.clone();
        for page in pages["pages"].as_array_mut().expect("pages") {
            if page["id"].as_str().is_some_and(|id| id.ends_with("/home")) {
                page["required"] = required.clone();
            }
        }
        pages
    };

    // The home page is required to show a composite of the digest and the personal OCR target.
    let mut composite_pack = sealed_pack.clone();
    composite_pack["schema_version"] = json!("0.7");
    let targets = composite_pack["targets"].as_array_mut().expect("targets");
    targets.push(json!({
        "type": "color_digest", "id": "digest/home_cafe",
        "region": {"x": 77, "y": 680, "width": 49, "height": 18},
        "algorithm": "color_digest.v1", "columns": 8, "rows": 8, "cells": cells,
        "exclude_cells": [], "max_mean_milli": 1500, "max_cell": 12
    }));
    targets.push(ocr.clone());
    targets.push(json!({
        "type": "composite", "id": "check/home_text", "mode": "all_of",
        "members": ["digest/home_cafe", "ocr/stamina"]
    }));
    observe(
        "composite(digest,personal ocr)",
        &rebuild(
            &cafe,
            &composite_pack,
            &home_pages(json!(["check/home_text", "ui/home_work_label"])),
            Some(&personal),
        ),
        &png,
        &frame_sha256,
    );

    // The same personal OCR target referenced by the page directly, in a 0.6 pack.
    let mut direct_pack = sealed_pack.clone();
    direct_pack["targets"]
        .as_array_mut()
        .expect("targets")
        .push(ocr);
    observe(
        "direct(personal ocr)",
        &rebuild(
            &cafe,
            &direct_pack,
            &home_pages(json!(["page/home", "ui/home_work_label", "ocr/stamina"])),
            Some(&personal),
        ),
        &png,
        &frame_sha256,
    );

    // The sealed 0.6 package as published.
    observe("sealed 0.6", &zip_entries(&cafe), &png, &frame_sha256);
}
