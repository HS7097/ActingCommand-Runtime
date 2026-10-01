// SPDX-License-Identifier: AGPL-3.0-only

//! One-off evidence for Workflow #308 D3, to be reverted. It runs `actinglab package dry-run`
//! and `actinglab lab validate` on the published bounty_tickets package (task schema 0.6, the
//! Lab 0.3-0.7 route) and on a copy whose point-click guard of bounty_select_max is a check
//! over the task's own MAX label template and MAX enabled color probe.

use actingcommand_contract::LabError;
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_pack_containment::Sha256Hash;
use actingcommand_pack_containment::source::{
    self, Bundle, OperationParser, ParseFiles, ParseOutputs, SourceFile, SourceRead,
};
use actingcommand_recognition::Scene;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

type Entries = BTreeMap<String, Vec<u8>>;

const ARCHIVE_URL: &str =
    "https://github.com/HS7097/ActingCommand/archive/536f048a3cac26ddbb0391895391f98967704cb0.zip";
const BUNDLE_SHA256: &str = "df524eac6290c4c0b435f3872a89e12dbe4f49e37555fa102bf96dd71b696af8";
const TASK: &str = "bounty_tickets";

fn emit(line: &str) {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "D3-LAB-ONEOFF {}", line.replace('\n', " | "))
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

fn sha256(bytes: &[u8]) -> String {
    Sha256Hash::digest(bytes).to_string()
}

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
    let coordinate_space = entry.data["coordinate_space"].clone();
    let defaults = entry.data["defaults"].clone();
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
/// documents and a manifest of hashes.
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
                .map(|relative| (relative.to_owned(), sha256(bytes)))
        })
        .collect::<BTreeMap<_, _>>();
    package.insert(
        "resources/manifest.json".to_owned(),
        to_bytes(&json!({"entry_task_id": entry_task_id, "hashes": hashes}))?,
    );
    Ok(zip_entries(&package))
}

/// A Home frame composed from the package's own Home template crops at their rectangles.
fn home_frame(entries: &Entries, task: &Value) -> Result<Vec<u8>, String> {
    let (width, height) = (1280_usize, 720_usize);
    let mut pixels = vec![0_u8; width * height * 3];
    for anchor in task["anchors"].as_array().ok_or("anchors missing")? {
        if !matches!(anchor["id"].as_str(), Some("home" | "home_work")) {
            continue;
        }
        let path = format!("resources/operations/{TASK}/{}", text(anchor, "template")?);
        let png = entries
            .get(&path)
            .ok_or_else(|| format!("missing {path}"))?;
        let scene = Scene::from_png(png).map_err(|error| error.message().to_owned())?;
        let rect = &anchor["region"]["rect"];
        let (x0, y0) = (
            rect["x"].as_u64().ok_or("x")? as usize,
            rect["y"].as_u64().ok_or("y")? as usize,
        );
        let template_width = scene.width() as usize;
        for (row, line) in scene.rgb8_pixels().chunks(template_width * 3).enumerate() {
            let start = ((y0 + row) * width + x0) * 3;
            pixels[start..start + line.len()].copy_from_slice(line);
        }
    }
    Frame::from_pixels(
        width as u32,
        height as u32,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::AdbScreencap,
    )
    .map_err(|error| error.to_string())?
    .png_for_artifact()
    .map_err(|error| error.to_string())
}

/// Replaces the build's own Runtime head, the only field that names the building commit.
fn masked(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, value) in object.iter_mut() {
                if key == "runtime_head" {
                    *value = json!("<runtime_head>");
                } else {
                    masked(value);
                }
            }
        }
        Value::Array(values) => values.iter_mut().for_each(masked),
        _ => {}
    }
}

fn actinglab(dir: &Path, args: &[&str]) -> (String, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_actinglab"))
        .args(args)
        .current_dir(dir)
        .env("ACTINGLAB_CONFIG_PATH", dir.join("config.json"))
        .output()
        .unwrap_or_else(|error| panic!("run actinglab: {error}"));
    let mut envelope: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        json!({
            "unparsed_stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
            "parse_error": error.to_string()
        })
    });
    masked(&mut envelope);
    (output.status.to_string(), envelope)
}

fn result_record(out: &Path) -> String {
    let Ok(bytes) = fs::read(out) else {
        return "no result zip".to_owned();
    };
    let entries = unzip(&bytes);
    let mut summary = Vec::new();
    for (name, data) in &entries {
        let rendered = match serde_json::from_slice::<Value>(data) {
            Ok(mut value) => {
                masked(&mut value);
                let bytes = serde_json::to_vec(&value).unwrap_or_default();
                format!("{name}:masked_sha256={}", sha256(&bytes))
            }
            Err(_) => format!("{name}:sha256={}", sha256(data)),
        };
        summary.push(rendered);
    }
    summary.join(",")
}

/// `package dry-run` with a Home fixture and `lab validate` on one ZIP.
fn exercise(name: &str, dir: &Path, package: &[u8], fixture: &Path) -> Vec<String> {
    let zip = format!("{name}.zip");
    let out = format!("{name}.result.zip");
    fs::write(dir.join(&zip), package).unwrap_or_else(|error| panic!("write {zip}: {error}"));
    let expected = sha256(package);
    let fixture = fixture.to_string_lossy().into_owned();
    let (status, dry_run) = actinglab(
        dir,
        &[
            "--json",
            "package",
            "dry-run",
            "--zip",
            zip.as_str(),
            "--expected-sha256",
            expected.as_str(),
            "--fixture",
            fixture.as_str(),
            "--out",
            out.as_str(),
        ],
    );
    let dry_run_text = dry_run.to_string();
    let (validate_status, validate) = actinglab(
        dir,
        &[
            "--json",
            "lab",
            "validate",
            "--zip",
            zip.as_str(),
            "--expected-sha256",
            expected.as_str(),
        ],
    );
    let validate_text = validate.to_string();
    vec![
        format!("{name} package_sha256={expected}"),
        format!(
            "{name} dry_run exit={status} masked_sha256={} result_zip={} output={dry_run_text}",
            sha256(dry_run_text.as_bytes()),
            result_record(&dir.join(&out))
        ),
        format!(
            "{name} lab_validate exit={validate_status} masked_sha256={} output={validate_text}",
            sha256(validate_text.as_bytes())
        ),
    ]
}

/// The copy: bounty_select_max is guarded by check/max_ready = all_of[MAX label template, MAX
/// enabled color], both already declared by the task and its bounty_select page rule.
fn check_guard_copy(entries: &Entries) -> Result<Vec<u8>, String> {
    let mut task = json_entry(entries, &task_path(TASK))?;
    let game = text(&task, "game")?;
    task["checks"] = json!([
        {"id": "check/max_ready", "all_of": ["ui/max_label", "state/max_enabled"]}
    ]);
    let operation = task["operations"]
        .as_array_mut()
        .ok_or("operations missing")?
        .iter_mut()
        .find(|operation| operation["id"].as_str() == Some("bounty_select_max"))
        .ok_or("bounty_select_max missing")?;
    let rect = operation["guard"]["expected_rect"].clone();
    operation["guard"] = json!({
        "page_id": format!("{game}/bounty_select"),
        "target_id": "check/max_ready",
        "expected_rect": rect,
        "check": "check/max_ready"
    });
    let mut copy = entries.clone();
    copy.insert(task_path(TASK), to_bytes(&task)?);
    let (outputs, canonical) = convert(&copy)?;
    let guard = outputs.primitives["primitives"]
        .as_array()
        .and_then(|rows| {
            rows.iter()
                .find(|row| row["id"].as_str() == Some("bounty_select_max"))
        })
        .map_or_else(|| "missing".to_owned(), |row| row["guard"].to_string());
    let check = outputs.pack["targets"]
        .as_array()
        .and_then(|rows| {
            rows.iter()
                .find(|row| row["id"].as_str() == Some("check/max_ready"))
        })
        .map_or_else(|| "missing".to_owned(), Value::to_string);
    emit(&format!(
        "check_guard_copy parsed pack_schema={} pages_schema={} check={check} guard={guard}",
        outputs.pack["schema_version"], outputs.pages["schema_version"]
    ));
    rebuild(&copy, &outputs, &canonical)
}

#[test]
fn one_off_d3_lab_accepts_a_check_guard_on_the_lab_route() {
    let archive = unzip(&download_archive());
    let (_, bundle_bytes) = archive
        .iter()
        .find(|(path, _)| path.contains("/bundles/") && path.ends_with(".zip"))
        .unwrap_or_else(|| panic!("no bundle in the archive"));
    assert_eq!(sha256(bundle_bytes), BUNDLE_SHA256);
    let bundle = unzip(bundle_bytes);
    let index = json_entry(&bundle, "bundle.json").unwrap_or_else(|error| panic!("{error}"));
    let (pack_path, sealed) = index["packs"]
        .as_array()
        .unwrap_or_else(|| panic!("bundle.json packs"))
        .iter()
        .filter_map(|pack| pack["path"].as_str())
        .map(|path| {
            let bytes = bundle.get(path).unwrap_or_else(|| panic!("missing {path}"));
            (path.to_owned(), bytes.clone())
        })
        .find(|(_, bytes)| {
            json_entry(&unzip(bytes), "control.json")
                .ok()
                .and_then(|control| control["entry_task_id"].as_str().map(str::to_owned))
                .is_some_and(|task| task == TASK)
        })
        .unwrap_or_else(|| panic!("no {TASK} package"));
    let entries = unzip(&sealed);
    let task = json_entry(&entries, &task_path(TASK)).unwrap_or_else(|error| panic!("{error}"));
    emit(&format!(
        "sealed {pack_path} sha256={} task_schema={}",
        sha256(&sealed),
        task["schema_version"]
    ));

    let dir = tempfile::TempDir::new().unwrap_or_else(|error| panic!("temp dir: {error}"));
    let fixture = dir.path().join("home.png");
    let frame = home_frame(&entries, &task).unwrap_or_else(|error| panic!("{error}"));
    fs::write(&fixture, &frame).unwrap_or_else(|error| panic!("write fixture: {error}"));
    emit(&format!("fixture home.png sha256={}", sha256(&frame)));

    for line in exercise("unchanged", dir.path(), &sealed, Path::new("home.png")) {
        emit(&line);
    }
    match check_guard_copy(&entries) {
        Ok(copy) => {
            for line in exercise("check_guard_copy", dir.path(), &copy, Path::new("home.png")) {
                emit(&line);
            }
        }
        Err(error) => emit(&format!("check_guard_copy parse=error {error}")),
    }
}
