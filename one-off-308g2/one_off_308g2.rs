// SPDX-License-Identifier: AGPL-3.0-only

//! One-off evidence for Workflow #308 G2 (model section 6, GC2), to be reverted. The one-off
//! workflow copies this file into the exact merge-base and into the PR head and runs it on both
//! in one job.
//!
//! C2: every task package of the published umbrella bundle is parsed through the source parser
//!     and the digests and schema versions of its derived documents are printed; the workflow
//!     compares these lines between the two builds.
//! L:  the list task's source form, recovered from its sealed package by undoing the one drag
//!     canonicalization, is derived without and with one added fixed_slots layout, and the
//!     derived package is admitted through the production task loader and the online
//!     observation preparation, both of which run the load-site page check.
//! M:  realistic authoring mistakes in that layout are refused with the pointer of the
//!     offending field.

use actingcommand_contract::LabError;
use actingcommand_execution_kernel::{
    ExternalExpectedSha256, PreparedContainedTask, PreparedPageObservation,
};
use actingcommand_pack_containment::Sha256Hash;
use actingcommand_pack_containment::source::{
    self, Bundle, OperationParser, ParseFiles, ParseOutputs, SourceFile, SourceRead,
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
const LIST_TASK: &str = "schedule_daily";
const LIST_PAGE: &str = "schedule_all";

fn emit(line: &str) {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "G2|{}", line.replace('\n', " | "))
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
fn refusal(stage: &str, error: LabError) -> String {
    format!(
        "stage={stage} code={} details={} message={}",
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

/// The source parse of one package's sources: the declaration gate (`resource validate`'s
/// task check), the parser's source validation, the full build and the canonical entry task.
/// Also reports whether the selected build of the entry task (`package build-task`) equals the
/// full build.
fn convert(entries: &Entries) -> Result<(ParseOutputs, Value, String), String> {
    let control = json_entry(entries, "control.json")?;
    let game = text(&control, "game")?;
    let server = text(&control, "server")?;
    let entry_task_id = text(&control, "entry_task_id")?;
    let resources = json_entry(entries, "resources/operations/resources.json")?;
    source::validate_resource_declarations(
        Path::new("resources/operations/resources.json"),
        &resources,
    )
    .map_err(|error| refusal("resources", error))?;
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
    source::declaration_file_requests(&bundles).map_err(|error| refusal("gate", error))?;
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
        game: source::canonical_game(&game).map_err(|error| refusal("metadata", error))?,
        server: source::canonical_server(&server).map_err(|error| refusal("metadata", error))?,
        locale: source::canonical_locale(&locale).map_err(|error| refusal("metadata", error))?,
        coordinate_space,
        defaults,
        resource_ids: source::resource_ids(&resources)
            .map_err(|error| refusal("metadata", error))?,
        existing_navigation: Some(json!({
            "control_points": resources
                .get("control_points")
                .cloned()
                .unwrap_or_else(|| json!([]))
        })),
        bundles,
        maa_task_overlays: HashMap::new(),
    };
    parser
        .validate_bundles(&files)
        .map_err(|error| refusal("validate", error))?;
    let outputs = parser
        .build_all(&files)
        .map_err(|error| refusal("build", error))?;
    let selected_equal = match parser.build_selected(std::slice::from_ref(&entry_task_id), &files)
    {
        Ok(selected) => [
            (&outputs.pack, &selected.pack),
            (&outputs.pages, &selected.pages),
            (&outputs.navigation, &selected.navigation),
            (&outputs.index, &selected.index),
            (&outputs.primitives, &selected.primitives),
        ]
        .iter()
        .all(|(full, selected)| full == selected)
        .to_string(),
        Err(error) => format!("error({})", refusal("build_selected", error)),
    };
    let canonical = parser
        .canonical_task(&entry_task_id)
        .map_err(|error| refusal("canonical", error))?;
    Ok((outputs, canonical, selected_equal))
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
/// documents and an in-memory manifest.
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

fn expected(zip: &[u8]) -> Result<ExternalExpectedSha256, String> {
    ExternalExpectedSha256::parse_hex(&Sha256Hash::digest(zip).to_string())
        .map_err(|error| format!("{error:?}"))
}

fn admit(zip: &[u8]) -> String {
    let expected = match expected(zip) {
        Ok(expected) => expected,
        Err(error) => return format!("error {error}"),
    };
    match PreparedContainedTask::load("one-off.instance", zip, expected) {
        Ok(_) => "ok".to_owned(),
        Err(error) => format!("error {error:?}"),
    }
}

fn observe(zip: &[u8]) -> String {
    let expected = match expected(zip) {
        Ok(expected) => expected,
        Err(error) => return format!("error {error}"),
    };
    match PreparedPageObservation::load("one-off.instance", zip, expected, &[], None) {
        Ok(_) => "ok".to_owned(),
        Err(error) => format!(
            "error code={} stage={} cause={}",
            error.code(),
            error.stage(),
            error.cause()
        ),
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

fn schemas(outputs: &ParseOutputs) -> String {
    format!(
        "pack_schema={} pages_schema={} navigation_schema={} index_schema={} primitives_schema={}",
        outputs.pack["schema_version"],
        outputs.pages["schema_version"],
        outputs.navigation["schema_version"],
        outputs.index["schema_version"],
        outputs.primitives["schema_version"]
    )
}

fn digests(outputs: &ParseOutputs) -> String {
    format!(
        "pack_json={} pages={} navigation={} index={} primitives={}",
        digest(&outputs.pack),
        digest(&outputs.pages),
        digest(&outputs.navigation),
        digest(&outputs.index),
        digest(&outputs.primitives)
    )
}

/// C2: schema versions and digests of every derived document of one sealed package, whether
/// they equal the sealed documents, and the admission of the rebuilt package.
fn describe(path: &str, entries: &Entries) -> String {
    match convert(entries) {
        Err(error) => format!("C2 pack={path} convert=error {error}"),
        Ok((outputs, canonical, selected_equal)) => {
            let projection = outputs
                .projection_metadata
                .as_ref()
                .map_or_else(|| "none".to_owned(), digest);
            let admitted = match rebuild(entries, &outputs, &canonical) {
                Ok(zip) => admit(&zip),
                Err(error) => format!("error {error}"),
            };
            format!(
                "C2 pack={path} {} {} projection={projection} canonical_task={} selected_equal={selected_equal} sealed_equal={} admitted={admitted}",
                schemas(&outputs),
                digests(&outputs),
                digest(&canonical),
                sealed_equality(entries, &outputs)
            )
        }
    }
}

/// The list task's source form: its sealed canonical task with every drag click's
/// `from_rect`/`to_rect` returned to the source fields `from`/`to`, the one canonicalization
/// the source grammar does not accept. Returns the package entries and the restored count.
fn source_form(package: &Entries) -> Result<(Entries, usize), String> {
    let mut task = json_entry(package, &task_path(LIST_TASK))?;
    let mut restored = 0;
    for operation in task
        .get_mut("operations")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "operations missing".to_owned())?
    {
        let Some(click) = operation.get_mut("click").and_then(Value::as_object_mut) else {
            continue;
        };
        if click.get("kind").and_then(Value::as_str) != Some("drag") {
            continue;
        }
        let from = click
            .remove("from_rect")
            .ok_or_else(|| "drag click without from_rect".to_owned())?;
        let to = click
            .remove("to_rect")
            .ok_or_else(|| "drag click without to_rect".to_owned())?;
        click.insert("from".to_owned(), from);
        click.insert("to".to_owned(), to);
        restored += 1;
    }
    let mut entries = package.clone();
    entries.insert(task_path(LIST_TASK), to_bytes(&task)?);
    Ok((entries, restored))
}

fn with_layouts(source: &Entries, layouts: &Value) -> Result<Entries, String> {
    let mut task = json_entry(source, &task_path(LIST_TASK))?;
    task["candidate_layouts"] = layouts.clone();
    let mut entries = source.clone();
    entries.insert(task_path(LIST_TASK), to_bytes(&task)?);
    Ok(entries)
}

/// One fixed_slots layout over three slots of the course list page: slot 0 reads the course
/// template the task already declares, the other two slots read nothing yet.
fn list_layout(page_id: &str) -> Value {
    let slot = |index: i64, targets: Value| {
        json!({
            "rect": {"x": 214 + 290 * index, "y": 204, "width": 176, "height": 90},
            "click": {"x": 224 + 290 * index, "y": 214, "width": 156, "height": 60},
            "targets": targets
        })
    };
    json!([{
        "id": "layout/course_slots",
        "page_id": page_id,
        "kind": "fixed_slots",
        "features": [
            {"name": "course", "value": "passed"},
            {"name": "course_score", "value": "measure_milli"}
        ],
        "slots": [
            slot(0, json!({"course": "ui/schedule_course_tower", "course_score": "ui/schedule_course_tower"})),
            slot(1, json!({})),
            slot(2, json!({}))
        ]
    }])
}

fn without(value: &Value, keys: &[&str]) -> Value {
    let mut value = value.clone();
    if let Some(object) = value.as_object_mut() {
        for key in keys {
            object.remove(*key);
        }
    }
    value
}

/// L: the list task's source form, then the same with only the layout added.
fn list_cases(package: &Entries) -> Result<Vec<String>, String> {
    let (source, restored) = source_form(package)?;
    let mut lines = vec![format!(
        "L source_form task={LIST_TASK} drag_clicks_restored={restored}"
    )];
    let base = match convert(&source) {
        Err(error) => {
            lines.push(format!("L source_form convert=error {error}"));
            None
        }
        Ok((outputs, canonical, selected_equal)) => {
            lines.push(format!(
                "L source_form convert=ok {} {} selected_equal={selected_equal} sealed_equal={} candidate_layouts_key={}",
                schemas(&outputs),
                digests(&outputs),
                sealed_equality(package, &outputs),
                outputs.pack.get("candidate_layouts").is_some()
            ));
            let admitted = rebuild(&source, &outputs, &canonical)
                .map_or_else(|error| format!("error {error}"), |zip| admit(&zip));
            lines.push(format!("L source_form admitted={admitted}"));
            Some(outputs)
        }
    };

    for (case, page_id) in [
        ("layout_added", LIST_PAGE.to_owned()),
        ("layout_added_full_page_id", format!("bluearchive/{LIST_PAGE}")),
    ] {
        let entries = with_layouts(&source, &list_layout(&page_id))?;
        match convert(&entries) {
            Err(error) => lines.push(format!("L {case} convert=error {error}")),
            Ok((outputs, canonical, selected_equal)) => {
                let others = base.as_ref().map_or_else(
                    || "no-base".to_owned(),
                    |base| {
                        format!(
                            "pages:{},navigation:{},index:{},primitives:{},pack_without_schema_and_layouts:{}",
                            base.pages == outputs.pages,
                            base.navigation == outputs.navigation,
                            base.index == outputs.index,
                            base.primitives == outputs.primitives,
                            without(&base.pack, &["schema_version"])
                                == without(&outputs.pack, &["schema_version", "candidate_layouts"])
                        )
                    },
                );
                lines.push(format!(
                    "L {case} convert=ok {} {} selected_equal={selected_equal} equal_to_source_form={others} canonical_task_keeps_layouts={}",
                    schemas(&outputs),
                    digests(&outputs),
                    canonical.get("candidate_layouts") == Some(&list_layout(&page_id))
                ));
                lines.push(format!(
                    "L {case} derived candidate_layouts={}",
                    outputs
                        .pack
                        .get("candidate_layouts")
                        .map_or_else(|| "absent".to_owned(), Value::to_string)
                ));
                let pages = outputs.pages["pages"]
                    .as_array()
                    .map(|pages| {
                        pages
                            .iter()
                            .filter_map(|page| page["id"].as_str())
                            .filter(|id| id.ends_with(LIST_PAGE))
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .unwrap_or_default();
                lines.push(format!("L {case} page_set_ids_for_list_page=[{pages}]"));
                match rebuild(&entries, &outputs, &canonical) {
                    Ok(zip) => {
                        lines.push(format!("L {case} admitted={}", admit(&zip)));
                        lines.push(format!(
                            "L {case} observation_preparation={}",
                            observe(&zip)
                        ));
                    }
                    Err(error) => lines.push(format!("L {case} rebuild=error {error}")),
                }
            }
        }
    }

    // M: realistic authoring mistakes.
    let layout = list_layout(LIST_PAGE);
    let mut misspelled_page = layout.clone();
    misspelled_page[0]["page_id"] = json!("schedule_al");
    let mut misspelled_target = layout.clone();
    misspelled_target[0]["slots"][0]["targets"]["course"] = json!("ui/schedule_course_towr");
    let mut copied_click = layout;
    copied_click[0]["slots"][2]["click"] =
        json!({"x": 1206, "y": 321, "width": 234, "height": 90});
    for (case, layouts) in [
        ("layout_page_misspelled", &misspelled_page),
        ("slot_0_target_misspelled", &misspelled_target),
        (
            "slot_2_click_copied_from_a_1920x1080_capture",
            &copied_click,
        ),
    ] {
        let entries = with_layouts(&source, layouts)?;
        match convert(&entries) {
            Err(error) => lines.push(format!("M {case} refused {error}")),
            Ok((outputs, _, _)) => lines.push(format!(
                "M {case} convert=ok (not refused) {}",
                schemas(&outputs)
            )),
        }
    }
    Ok(lines)
}

#[test]
fn one_off_308g2_candidate_layouts_source_family() {
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
    let list = packs
        .iter()
        .map(|(_, entries)| entries)
        .find(|entries| entry_task_id(entries).is_ok_and(|id| id == LIST_TASK))
        .unwrap_or_else(|| panic!("no package for {LIST_TASK}"));
    for line in list_cases(list).unwrap_or_else(|error| panic!("{error}")) {
        emit(&line);
    }
}
