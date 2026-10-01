// SPDX-License-Identifier: AGPL-3.0-only

//! One-off evidence for Workflow #308 D3, to be reverted. C2 parses every task package of a
//! published bundle through the source parser and prints digests of the derived documents.
//! C3 admits copies of one package that declare a check, color digests, a digest guard and a
//! per-target color threshold, and refuses two realistic authoring mistakes.

use actingcommand_contract::LabError;
use actingcommand_execution_kernel::{ExternalExpectedSha256, PreparedContainedTask};
use actingcommand_pack_containment::Sha256Hash;
use actingcommand_pack_containment::source::{
    self, Bundle, OperationParser, ParseFiles, ParseOutputs, SourceFile, SourceRead,
};
use actingcommand_recognition::color_digest::{ColorDigest, ColorDigestGrid};
use actingcommand_recognition::{Rect, Scene};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

type Entries = BTreeMap<String, Vec<u8>>;

const ARCHIVE_URL: &str =
    "https://github.com/HS7097/ActingCommand/archive/536f048a3cac26ddbb0391895391f98967704cb0.zip";
const BUNDLE_SHA256: &str = "df524eac6290c4c0b435f3872a89e12dbe4f49e37555fa102bf96dd71b696af8";
const BASE_TASK: &str = "cafe_income";
const OCR_TASK: &str = "home_readings";

fn emit(line: &str) {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "D3-ONEOFF {}", line.replace('\n', " | "))
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

fn json_entry(entries: &Entries, path: &str) -> Result<Value, String> {
    let bytes = entries.get(path).ok_or_else(|| format!("missing {path}"))?;
    serde_json::from_slice(bytes).map_err(|error| format!("{path}: {error}"))
}

fn text(value: &Value, key: &str) -> Result<String, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("missing string {key}"))
}

fn to_bytes(value: &Value) -> Result<Vec<u8>, String> {
    serde_json::to_vec(value).map_err(|error| error.to_string())
}

fn digest(value: &Value) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_else(|error| panic!("encode: {error}"));
    Sha256Hash::digest(&bytes).to_string()
}

/// The code, the declaration issue (file and JSON pointer) and the message of a refusal.
fn refusal(error: LabError) -> String {
    format!(
        "code={} details={} message={}",
        error.code,
        error
            .details
            .as_ref()
            .map_or_else(|| "none".to_owned(), Value::to_string),
        error.message
    )
}

fn task_path(task: &str) -> String {
    format!("resources/operations/{task}/task.json")
}

fn entry_task_id(entries: &Entries) -> Result<String, String> {
    text(&json_entry(entries, "control.json")?, "entry_task_id")
}

/// The in-memory parse the source loader performs, over one sealed package's sources.
fn convert(entries: &Entries) -> Result<(ParseOutputs, Value), String> {
    let control = json_entry(entries, "control.json")?;
    let game = text(&control, "game")?;
    let server = text(&control, "server")?;
    let entry_task_id = text(&control, "entry_task_id")?;
    let resources = json_entry(entries, "resources/operations/resources.json")?;
    source::validate_resource_declarations(
        Path::new("resources/operations/resources.json"),
        &resources,
    )
    .map_err(refusal)?;
    let mut bundles = Vec::new();
    for (path, bytes) in entries {
        let Some(task) = path
            .strip_prefix("resources/operations/")
            .and_then(|path| path.strip_suffix("/task.json"))
        else {
            continue;
        };
        if task.contains('/') {
            return Err(format!("nested task path {path}"));
        }
        bundles.push(Bundle {
            task_id: task.to_owned(),
            dir: PathBuf::from(format!("resources/operations/{task}")),
            data: serde_json::from_slice(bytes).map_err(|error| format!("{path}: {error}"))?,
        });
    }
    source::declaration_file_requests(&bundles).map_err(refusal)?;
    let entry = bundles
        .iter()
        .find(|bundle| bundle.task_id == entry_task_id)
        .ok_or_else(|| format!("missing entry task {entry_task_id}"))?;
    let coordinate_space = entry
        .data
        .get("coordinate_space")
        .cloned()
        .ok_or_else(|| "entry coordinate_space missing".to_owned())?;
    let defaults = entry
        .data
        .get("defaults")
        .cloned()
        .ok_or_else(|| "entry defaults missing".to_owned())?;
    let locale = text(&entry.data, "locale")?;
    let projection_path = format!("resources/navigation/{game}.{server}.projection.json");
    let files = ParseFiles {
        files: Arc::new(
            source::source_file_requests(&bundles)
                .into_iter()
                .map(|(path, read)| {
                    let key = path.to_string_lossy().replace('\\', "/");
                    let file = match entries.get(&key) {
                        Some(bytes) => SourceFile {
                            is_file: true,
                            length: Ok(bytes.len() as u64),
                            bytes: match read {
                                SourceRead::Metadata => Err("metadata-only parse input".to_owned()),
                                SourceRead::BoundedBytes(limit) if bytes.len() as u64 > limit => {
                                    Err("parse input exceeds declared limit".to_owned())
                                }
                                SourceRead::Bytes | SourceRead::BoundedBytes(_) => {
                                    Ok(bytes.clone())
                                }
                            },
                        },
                        None => SourceFile {
                            is_file: false,
                            length: Err("source dependency missing".to_owned()),
                            bytes: Err("source dependency missing".to_owned()),
                        },
                    };
                    (path, file)
                })
                .collect(),
        ),
        projection_bytes: Ok(entries.get(&projection_path).cloned()),
        projection_exists: Ok(entries.contains_key(&projection_path)),
    };
    let parser = OperationParser {
        root: PathBuf::from("resources"),
        game: source::canonical_game(&game).map_err(refusal)?,
        server: source::canonical_server(&server).map_err(refusal)?,
        locale: source::canonical_locale(&locale).map_err(refusal)?,
        coordinate_space,
        defaults,
        resource_ids: source::resource_ids(&resources).map_err(refusal)?,
        existing_navigation: Some(json!({
            "control_points": resources
                .get("control_points")
                .cloned()
                .unwrap_or_else(|| json!([]))
        })),
        bundles,
        maa_task_overlays: HashMap::new(),
    };
    parser.validate_bundles(&files).map_err(refusal)?;
    let outputs = parser.build_all(&files).map_err(refusal)?;
    let canonical = parser.canonical_task(&entry_task_id).map_err(refusal)?;
    Ok((outputs, canonical))
}

fn is_derived(path: &str) -> bool {
    path == "resources/manifest.json"
        || path.ends_with(".pack.json")
        || path.ends_with(".pages.json")
        || path.ends_with(".navigation.json")
        || path.ends_with("operations.index.json")
        || path.ends_with("operations.primitives.json")
}

/// The package the source loader issues: sources, the canonical entry task, the five derived
/// documents and an in-memory manifest, admitted through the production task loader.
fn rebuild(
    entries: &Entries,
    outputs: &ParseOutputs,
    canonical: &Value,
) -> Result<Vec<u8>, String> {
    let control = json_entry(entries, "control.json")?;
    let stem = format!("{}.{}", text(&control, "game")?, text(&control, "server")?);
    let entry_task_id = text(&control, "entry_task_id")?;
    let mut package = entries.clone();
    package.retain(|path, _| !is_derived(path));
    package.insert(task_path(&entry_task_id), to_bytes(canonical)?);
    for (path, value) in [
        (
            format!("resources/recognition/{stem}.pack.json"),
            &outputs.pack,
        ),
        (
            format!("resources/recognition/{stem}.pages.json"),
            &outputs.pages,
        ),
        (
            format!("resources/navigation/{stem}.navigation.json"),
            &outputs.navigation,
        ),
        (
            "resources/operations/operations.index.json".to_owned(),
            &outputs.index,
        ),
        (
            "resources/operations/operations.primitives.json".to_owned(),
            &outputs.primitives,
        ),
    ] {
        package.insert(path, to_bytes(value)?);
    }
    let hashes = package
        .iter()
        .filter_map(|(path, bytes)| {
            path.strip_prefix("resources/")
                .map(|relative| (relative.to_owned(), Sha256Hash::digest(bytes).to_string()))
        })
        .collect::<BTreeMap<_, _>>();
    package.insert(
        "resources/manifest.json".to_owned(),
        to_bytes(&json!({"entry_task_id": entry_task_id, "hashes": hashes}))?,
    );
    Ok(zip_entries(&package))
}

fn admit(zip: &[u8]) -> Result<(), String> {
    let expected = ExternalExpectedSha256::parse_hex(&Sha256Hash::digest(zip).to_string())
        .map_err(|error| format!("{error:?}"))?;
    PreparedContainedTask::load("one-off.instance", zip, expected)
        .map(|_| ())
        .map_err(|error| format!("{error:?}"))
}

fn verdict(result: Result<(), String>) -> String {
    match result {
        Ok(()) => "ok".to_owned(),
        Err(error) => format!("error {error}"),
    }
}

fn sealed_equality(entries: &Entries, outputs: &ParseOutputs) -> String {
    [
        (".pack.json", &outputs.pack),
        (".pages.json", &outputs.pages),
        (".navigation.json", &outputs.navigation),
        ("operations.index.json", &outputs.index),
        ("operations.primitives.json", &outputs.primitives),
    ]
    .into_iter()
    .map(|(suffix, value)| {
        let sealed = entries
            .iter()
            .find(|(path, _)| path.ends_with(suffix))
            .and_then(|(_, bytes)| serde_json::from_slice::<Value>(bytes).ok());
        format!("{suffix}:{}", sealed.as_ref() == Some(value))
    })
    .collect::<Vec<_>>()
    .join(",")
}

/// C2: digests of every derived document of one sealed package, and its admission.
fn describe(path: &str, entries: &Entries) -> String {
    match convert(entries) {
        Err(error) => format!("C2 pack={path} convert=error {error}"),
        Ok((outputs, canonical)) => {
            let projection = outputs
                .projection_metadata
                .as_ref()
                .map_or_else(|| "none".to_owned(), digest);
            let admitted = rebuild(entries, &outputs, &canonical).and_then(|zip| admit(&zip));
            format!(
                "C2 pack={path} pack_schema={} pack_json={} pages_schema={} pages={} navigation={} index={} primitives={} projection={projection} canonical_task={} sealed_equal={} admitted={}",
                outputs.pack["schema_version"],
                digest(&outputs.pack),
                outputs.pages["schema_version"],
                digest(&outputs.pages),
                digest(&outputs.navigation),
                digest(&outputs.index),
                digest(&outputs.primitives),
                digest(&canonical),
                sealed_equality(entries, &outputs),
                verdict(admitted)
            )
        }
    }
}

fn page_definition(pages: &Value, page: &str) -> String {
    let suffix = format!("/{page}");
    pages["pages"]
        .as_array()
        .and_then(|pages| {
            pages
                .iter()
                .find(|entry| entry["id"].as_str().is_some_and(|id| id.ends_with(&suffix)))
        })
        .map_or_else(|| "missing".to_owned(), Value::to_string)
}

fn find_by_id<'a>(rows: &'a Value, key: &str, id: &str) -> Option<&'a Value> {
    rows[key]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["id"].as_str() == Some(id)))
}

/// C3: one copy of the base package, its derived targets, home page, guard and admission.
fn c3_outcome(name: &str, entries: &Entries, shown: &[&str], operation: Option<&str>) -> String {
    match convert(entries) {
        Err(error) => format!("C3 {name} convert=error {error}"),
        Ok((outputs, canonical)) => {
            let targets = shown
                .iter()
                .map(|id| {
                    find_by_id(&outputs.pack, "targets", id)
                        .map_or_else(|| format!("{id}:missing"), Value::to_string)
                })
                .collect::<Vec<_>>()
                .join(" ");
            let guard = operation.map_or_else(
                || "-".to_owned(),
                |operation| {
                    find_by_id(&outputs.primitives, "primitives", operation)
                        .map_or_else(|| "missing".to_owned(), |row| row["guard"].to_string())
                },
            );
            let admitted = rebuild(entries, &outputs, &canonical).and_then(|zip| admit(&zip));
            format!(
                "C3 {name} convert=ok pack_schema={} pages_schema={} navigation_schema={} index_schema={} primitives_schema={} home_page={} targets={targets} guard={guard} admitted={}",
                outputs.pack["schema_version"],
                outputs.pages["schema_version"],
                outputs.navigation["schema_version"],
                outputs.index["schema_version"],
                outputs.primitives["schema_version"],
                page_definition(&outputs.pages, "home"),
                verdict(admitted)
            )
        }
    }
}

/// The real digest of a packaged template image, which is the frame crop of its rectangle.
fn template_digest(
    entries: &Entries,
    asset: &str,
    columns: u32,
    rows: u32,
) -> Result<String, String> {
    let path = format!("resources/operations/{BASE_TASK}/{asset}");
    let png = entries
        .get(&path)
        .ok_or_else(|| format!("missing {path}"))?;
    let scene = Scene::from_png(png).map_err(|error| error.message().to_owned())?;
    let width = i32::try_from(scene.width()).map_err(|error| error.to_string())?;
    let height = i32::try_from(scene.height()).map_err(|error| error.to_string())?;
    let grid = ColorDigestGrid::new(columns, rows).map_err(|error| error.to_string())?;
    ColorDigest::compute(
        &scene,
        Rect {
            x: 0,
            y: 0,
            width,
            height,
        },
        grid,
    )
    .map(|digest| digest.to_hex())
    .map_err(|error| error.to_string())
}

fn verify_template(task: &Value, id: &str) -> Result<(String, Value), String> {
    let entry = find_by_id(task, "verify_templates", id).ok_or_else(|| format!("missing {id}"))?;
    Ok((text(entry, "template")?, entry["region"]["rect"].clone()))
}

fn digest_probe(id: &str, rect: &Value, columns: u32, rows: u32, cells: &str) -> Value {
    json!({
        "id": id,
        "region": {"mode": "rect", "rect": rect},
        "digest": {
            "algorithm": "color_digest.v1",
            "columns": columns,
            "rows": rows,
            "cells": cells,
            "max_mean_milli": 1500,
            "max_cell": 12
        }
    })
}

fn array_mut<'a>(value: &'a mut Value, key: &str) -> Result<&'a mut Vec<Value>, String> {
    value
        .get_mut(key)
        .and_then(Value::as_array_mut)
        .ok_or_else(|| format!("{key} missing"))
}

fn with_task(base: &Entries, task: &Value) -> Result<Entries, String> {
    let mut entries = base.clone();
    entries.insert(task_path(BASE_TASK), to_bytes(task)?);
    Ok(entries)
}

fn c3(base: &Entries, stamina: &Value) -> Result<Vec<String>, String> {
    let task = json_entry(base, &task_path(BASE_TASK))?;
    let game = text(&task, "game")?;
    let (home_asset, home_rect) = verify_template(&task, "ui/home_cafe_label")?;
    let home_cells = template_digest(base, &home_asset, 8, 8)?;
    let home_digest = digest_probe("digest/home_cafe_label", &home_rect, 8, 8, &home_cells);
    let (return_asset, return_rect) = verify_template(&task, "ui/return_home_icon")?;
    let return_cells = template_digest(base, &return_asset, 4, 4)?;
    let return_digest = digest_probe("digest/return_home", &return_rect, 4, 4, &return_cells);
    let mut lines = vec![format!(
        "C3 digests home_cafe_label 8x8 {home_rect} {home_cells} return_home 4x4 {return_rect} {return_cells}"
    )];

    // 1. Home is declared only by check(digest and OCR); the home template anchor is gone.
    let mut digest_and_ocr = task.clone();
    array_mut(&mut digest_and_ocr, "anchors")?
        .retain(|anchor| anchor["id"].as_str() != Some("home"));
    array_mut(&mut digest_and_ocr, "color_probes")?.push(home_digest.clone());
    digest_and_ocr["ocr_targets"] = json!([stamina]);
    digest_and_ocr["checks"] = json!([
        {"id": "check/home", "all_of": ["digest/home_cafe_label", "ocr/stamina"]}
    ]);
    digest_and_ocr["page_rules"]["home"]["required"] = json!(["check/home"]);
    lines.push(c3_outcome(
        "home_check_digest_and_ocr",
        &with_task(base, &digest_and_ocr)?,
        &["digest/home_cafe_label", "ocr/stamina", "check/home"],
        None,
    ));

    // 2. Home is declared by check(digest and page/home); the anchor stays.
    let mut digest_and_page = task.clone();
    array_mut(&mut digest_and_page, "color_probes")?.push(home_digest);
    digest_and_page["checks"] = json!([
        {"id": "check/home", "all_of": ["digest/home_cafe_label", "page/home"]}
    ]);
    digest_and_page["page_rules"]["home"]["required"] = json!(["check/home"]);
    lines.push(c3_outcome(
        "home_check_digest_and_page",
        &with_task(base, &digest_and_page)?,
        &["digest/home_cafe_label", "check/home"],
        None,
    ));

    // 3. The point-click return step is guarded by a digest of its own button region.
    let mut digest_guard = task.clone();
    array_mut(&mut digest_guard, "color_probes")?.push(return_digest);
    let operation = array_mut(&mut digest_guard, "operations")?
        .iter_mut()
        .find(|operation| operation["id"].as_str() == Some("cafe_return_home"))
        .ok_or_else(|| "cafe_return_home missing".to_owned())?;
    operation["guard"] = json!({
        "page_id": format!("{game}/cafe"),
        "target_id": "digest/return_home",
        "expected_rect": return_rect,
        "color_probe": "digest/return_home"
    });
    lines.push(c3_outcome(
        "guard_color_probe_digest",
        &with_task(base, &digest_guard)?,
        &["digest/return_home"],
        Some("cafe_return_home"),
    ));

    // 4. One color probe carries its own threshold.
    let mut probe_threshold = task.clone();
    array_mut(&mut probe_threshold, "color_probes")?
        .iter_mut()
        .find(|probe| probe["id"].as_str() == Some("state/cafe_receive_ready"))
        .ok_or_else(|| "state/cafe_receive_ready missing".to_owned())?["max_distance"] = json!(30);
    lines.push(c3_outcome(
        "color_probe_max_distance",
        &with_task(base, &probe_threshold)?,
        &["state/cafe_receive_ready", "state/cafe_receive_empty"],
        None,
    ));

    // Mistake 1: the grid was changed to 4x8 but the cells were not recomputed.
    let mut grid_changed = digest_and_ocr.clone();
    array_mut(&mut grid_changed, "color_probes")?
        .last_mut()
        .ok_or_else(|| "digest entry missing".to_owned())?["digest"]["columns"] = json!(4);
    lines.push(c3_outcome(
        "mistake_grid_changed_cells_stale",
        &with_task(base, &grid_changed)?,
        &[],
        None,
    ));

    // Mistake 2: a check member ID is misspelled.
    let mut misspelled = digest_and_ocr;
    misspelled["checks"][0]["all_of"][1] = json!("ocr/stamnia");
    lines.push(c3_outcome(
        "mistake_member_misspelled",
        &with_task(base, &misspelled)?,
        &[],
        None,
    ));
    Ok(lines)
}

#[test]
fn one_off_d3_source_checks_and_derived_identity() {
    let archive = unzip(&download_archive());
    let (bundle_name, bundle_bytes) = archive
        .iter()
        .find(|(path, _)| path.contains("/bundles/") && path.ends_with(".zip"))
        .unwrap_or_else(|| panic!("no bundle in the archive"));
    let bundle_sha256 = Sha256Hash::digest(bundle_bytes).to_string();
    assert_eq!(bundle_sha256, BUNDLE_SHA256, "{bundle_name}");
    let bundle = unzip(bundle_bytes);
    let index = json_entry(&bundle, "bundle.json").unwrap_or_else(|error| panic!("{error}"));
    let mut packs = Vec::new();
    for pack in index["packs"]
        .as_array()
        .unwrap_or_else(|| panic!("bundle.json packs"))
    {
        let path = pack["path"].as_str().unwrap_or_else(|| panic!("pack path"));
        let bytes = bundle.get(path).unwrap_or_else(|| panic!("missing {path}"));
        let actual = Sha256Hash::digest(bytes).to_string();
        let expected = pack["sha256"]
            .as_str()
            .unwrap_or_else(|| panic!("pack sha256 {path}"));
        assert_eq!(actual, expected, "{path}");
        packs.push((path.to_owned(), unzip(bytes)));
    }
    emit(&format!("bundle={bundle_sha256} packs={}", packs.len()));
    for (path, entries) in &packs {
        emit(&describe(path, entries));
    }

    let package = |task: &str| {
        packs
            .iter()
            .map(|(_, entries)| entries)
            .find(|entries| entry_task_id(entries).is_ok_and(|id| id == task))
            .unwrap_or_else(|| panic!("no package for {task}"))
    };
    let readings = json_entry(package(OCR_TASK), &task_path(OCR_TASK))
        .unwrap_or_else(|error| panic!("{error}"));
    let stamina = find_by_id(&readings, "ocr_targets", "ocr/stamina")
        .cloned()
        .unwrap_or_else(|| panic!("no ocr/stamina declaration"));
    for line in c3(package(BASE_TASK), &stamina).unwrap_or_else(|error| panic!("{error}")) {
        emit(&line);
    }
}
