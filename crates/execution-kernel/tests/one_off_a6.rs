// SPDX-License-Identifier: AGPL-3.0-only

//! One-off evidence for Workflow #288 A6, to be reverted. It converts every task package of a
//! published bundle through the source converter, prints digests of the derived documents,
//! and admits copies whose target page is declared by color (or color and OCR) targets only.

use actingcommand_execution_kernel::{ExternalExpectedSha256, PreparedContainedTask};
use actingcommand_pack_containment::Sha256Hash;
use actingcommand_pack_containment::source::{
    self, Bundle, ConversionFiles, ConvertOutputs, OperationConverter, SourceFile, SourceRead,
};
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
const COLOR_TASK: &str = "resources/operations/battle_auto_enable/task.json";
const COLOR_PAGE: &str = "battle_auto_on";

fn emit(line: &str) {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "A6-ONEOFF {}", line.replace('\n', " | "))
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

/// The in-memory conversion the source loader performs, over one sealed package's sources.
fn convert(entries: &Entries) -> Result<(ConvertOutputs, Value), String> {
    let control = json_entry(entries, "control.json")?;
    let game = text(&control, "game")?;
    let server = text(&control, "server")?;
    let entry_task_id = text(&control, "entry_task_id")?;
    let resources = json_entry(entries, "resources/operations/resources.json")?;
    source::validate_resource_declarations(
        Path::new("resources/operations/resources.json"),
        &resources,
    )
    .map_err(|error| error.message)?;
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
    source::declaration_file_requests(&bundles).map_err(|error| error.message)?;
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
    let files = ConversionFiles {
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
                                SourceRead::Metadata => {
                                    Err("metadata-only conversion input".to_owned())
                                }
                                SourceRead::BoundedBytes(limit) if bytes.len() as u64 > limit => {
                                    Err("conversion input exceeds declared limit".to_owned())
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
    let converter = OperationConverter {
        root: PathBuf::from("resources"),
        game: source::canonical_game(&game).map_err(|error| error.message)?,
        server: source::canonical_server(&server).map_err(|error| error.message)?,
        locale: source::canonical_locale(&locale).map_err(|error| error.message)?,
        coordinate_space,
        defaults,
        resource_ids: source::resource_ids(&resources).map_err(|error| error.message)?,
        existing_navigation: Some(json!({
            "control_points": resources
                .get("control_points")
                .cloned()
                .unwrap_or_else(|| json!([]))
        })),
        bundles,
        maa_task_overlays: HashMap::new(),
    };
    converter
        .validate_bundles(&files)
        .map_err(|error| error.message)?;
    let outputs = converter.build_all(&files).map_err(|error| error.message)?;
    let canonical = converter
        .canonical_task(&entry_task_id)
        .map_err(|error| error.message)?;
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
    outputs: &ConvertOutputs,
    canonical: &Value,
) -> Result<Vec<u8>, String> {
    let control = json_entry(entries, "control.json")?;
    let stem = format!("{}.{}", text(&control, "game")?, text(&control, "server")?);
    let entry_task_id = text(&control, "entry_task_id")?;
    let mut package = entries.clone();
    package.retain(|path, _| !is_derived(path));
    package.insert(
        format!("resources/operations/{entry_task_id}/task.json"),
        to_bytes(canonical)?,
    );
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

fn sealed_equality(entries: &Entries, outputs: &ConvertOutputs) -> String {
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

fn describe(path: &str, entries: &Entries) -> String {
    match convert(entries) {
        Err(error) => format!("pack={path} convert=error {error}"),
        Ok((outputs, canonical)) => {
            let projection = outputs
                .projection_metadata
                .as_ref()
                .map_or_else(|| "none".to_owned(), digest);
            let admitted = rebuild(entries, &outputs, &canonical).and_then(|zip| admit(&zip));
            format!(
                "pack={path} pack_json={} pages={} navigation={} index={} primitives={} projection={projection} canonical_task={} sealed_equal={} admitted={}",
                digest(&outputs.pack),
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

fn outcome(entries: &Entries) -> String {
    match convert(entries) {
        Err(error) => format!("convert=error {error}"),
        Ok((outputs, canonical)) => {
            let admitted = rebuild(entries, &outputs, &canonical).and_then(|zip| admit(&zip));
            format!(
                "convert=ok page={} admitted={}",
                page_definition(&outputs.pages, COLOR_PAGE),
                verdict(admitted)
            )
        }
    }
}

/// The target page loses its template anchor and is declared by its page rule alone.
fn anchorless_variant(
    entries: &Entries,
    rule: Value,
    ocr: Option<Value>,
) -> Result<Entries, String> {
    let mut task = json_entry(entries, COLOR_TASK)?;
    let anchors = task
        .get_mut("anchors")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "anchors missing".to_owned())?;
    let count = anchors.len();
    anchors.retain(|anchor| anchor.get("id").and_then(Value::as_str) != Some(COLOR_PAGE));
    if anchors.len() + 1 != count {
        return Err(format!("anchor {COLOR_PAGE} not found"));
    }
    task["page_rules"][COLOR_PAGE] = rule;
    if let Some(ocr) = ocr {
        task["ocr_targets"] = json!([ocr]);
    }
    let mut variant = entries.clone();
    variant.insert(COLOR_TASK.to_owned(), to_bytes(&task)?);
    Ok(variant)
}

fn first_ocr_target(entries: &Entries) -> Option<Value> {
    entries
        .iter()
        .filter(|(path, _)| path.ends_with("/task.json"))
        .filter_map(|(_, bytes)| serde_json::from_slice::<Value>(bytes).ok())
        .find_map(|task| {
            task["ocr_targets"]
                .as_array()
                .and_then(|targets| targets.first())
                .cloned()
        })
}

#[test]
fn one_off_a6_page_backends_are_not_forced() {
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

    let base = packs
        .iter()
        .map(|(_, entries)| entries)
        .find(|entries| entries.contains_key(COLOR_TASK))
        .unwrap_or_else(|| panic!("no package holds {COLOR_TASK}"));
    let ocr = packs
        .iter()
        .find_map(|(_, entries)| first_ocr_target(entries))
        .unwrap_or_else(|| panic!("no packaged OCR target"));
    emit(&format!("E8b original {}", outcome(base)));

    let color_only = anchorless_variant(
        base,
        json!({"required": ["state/auto_on"], "forbidden": ["state/auto_off"]}),
        None,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    emit(&format!("E8b color_only {}", outcome(&color_only)));

    let mut label = ocr;
    label["id"] = json!("text/auto_label");
    label["region"] =
        json!({"mode": "rect", "rect": {"x": 1180, "y": 664, "width": 73, "height": 25}});
    label["expected"] = json!(["AUTO"]);
    label["minimum_confidence"] = json!(0.5);
    let color_and_ocr = anchorless_variant(
        base,
        json!({"required": ["state/auto_on", "text/auto_label"], "forbidden": ["state/auto_off"]}),
        Some(label),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    emit(&format!("E8b color_and_ocr {}", outcome(&color_and_ocr)));
}
