// SPDX-License-Identifier: AGPL-3.0-only

//! One-off evidence for Workflow #308 G4 (model section 6, GC4, last clause), to be reverted.
//! The one-off workflow copies this file into the exact merge-base and into the PR head and runs
//! it on both in one job; the workflow compares every `G4C|` line.
//!
//! D: every task package of the published umbrella bundle (none has a select step) is parsed
//!    from its sources; the schema versions and digests of the five derived documents and of
//!    the canonical task are printed, and the sealed package is admitted.
//! T: every package runs in the kernel on two black frames; the full trace is printed as a
//!    digest, its length and the run's end.
//! S: the unmodified schedule package runs through its click path on frames composed of its own
//!    images at their declared rectangles (home, list, area, the course list with its course
//!    card, the course detail) until the input after the course click; every trace is printed.

use actingcommand_contract::{InputAction, LabError};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_execution_kernel::{
    ContainedTaskRunError, ContainedTaskRuntime, ContainedTaskRuntimeErrorClass,
    ContainedTaskTrace, ExternalExpectedSha256, InputFrameContext, ObservedFrame,
    PreparedContainedTask,
};
use actingcommand_pack_containment::Sha256Hash;
use actingcommand_pack_containment::source::{
    self, Bundle, OperationParser, ParseFiles, ParseOutputs, SourceFile,
};
use actingcommand_recognition::Scene;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

type Entries = BTreeMap<String, Vec<u8>>;

const ARCHIVE_URL: &str =
    "https://github.com/HS7097/ActingCommand/archive/536f048a3cac26ddbb0391895391f98967704cb0.zip";
const BUNDLE_SHA256: &str = "df524eac6290c4c0b435f3872a89e12dbe4f49e37555fa102bf96dd71b696af8";
const LIST_TASK: &str = "schedule_daily";
const WIDTH: usize = 1280;
const HEIGHT: usize = 720;

fn emit(line: &str) {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "G4C|{}", line.replace('\n', " | "))
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

fn digest_bytes(bytes: &[u8]) -> String {
    Sha256Hash::digest(bytes).to_string()
}

fn digest(value: &Value) -> String {
    digest_bytes(&serde_json::to_vec(value).unwrap_or_else(|error| panic!("encode: {error}")))
}

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

/// The source parse of one package's sources, as the content-directory loader parses them.
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
    .map_err(|error| refusal("resources", error))?;
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
    source::declaration_file_requests(&bundles).map_err(|error| refusal("gate", error))?;
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
                .map(|(path, _)| {
                    let key = path.to_string_lossy().replace('\\', "/");
                    let file = match entries.get(&key) {
                        Some(bytes) => SourceFile {
                            is_file: true,
                            length: Ok(bytes.len() as u64),
                            bytes: Ok(bytes.clone()),
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
            "control_points": resources.get("control_points").cloned().unwrap_or_else(|| json!([]))
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
    let canonical = parser
        .canonical_task(&entry_task_id)
        .map_err(|error| refusal("canonical", error))?;
    Ok((outputs, canonical))
}

/// The package's sources: each sealed task with every drag click's `from_rect` / `to_rect`
/// returned to the source fields `from` / `to`, the one canonicalization the source grammar
/// does not accept (G2 one-off).
fn source_entries(entries: &Entries) -> Entries {
    let mut sources = entries.clone();
    for (path, bytes) in sources.iter_mut() {
        if !(path.starts_with("resources/operations/") && path.ends_with("/task.json")) {
            continue;
        }
        let mut task: Value = serde_json::from_slice(bytes).expect("task json");
        let mut restored = 0;
        for operation in task["operations"].as_array_mut().into_iter().flatten() {
            let Some(click) = operation.get_mut("click").and_then(Value::as_object_mut) else {
                continue;
            };
            if click.get("kind").and_then(Value::as_str) != Some("drag") {
                continue;
            }
            if let (Some(from), Some(to)) = (click.remove("from_rect"), click.remove("to_rect")) {
                click.insert("from".to_owned(), from);
                click.insert("to".to_owned(), to);
                restored += 1;
            }
        }
        if restored > 0 {
            *bytes = serde_json::to_vec(&task).expect("task bytes");
        }
    }
    sources
}

fn expected(zip: &[u8]) -> ExternalExpectedSha256 {
    ExternalExpectedSha256::parse_hex(&digest_bytes(zip))
        .unwrap_or_else(|error| panic!("{error:?}"))
}

/// Serves composed frames, records every trace, and ends the run at a given input.
struct ScriptedRuntime {
    frames: VecDeque<Frame>,
    traces: Vec<String>,
    inputs: Vec<String>,
    stop_at_input: usize,
}

#[derive(Debug)]
enum Scripted {
    FramesExhausted,
    StoppedAtInput,
}

impl ContainedTaskRuntime for ScriptedRuntime {
    type Error = Scripted;

    fn classify_error(_error: &Self::Error) -> ContainedTaskRuntimeErrorClass {
        ContainedTaskRuntimeErrorClass::Nonfatal
    }

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        self.frames
            .pop_front()
            .map(ObservedFrame::from)
            .ok_or(Scripted::FramesExhausted)
    }

    fn action_seed(
        &mut self,
        step_index: u32,
        _operation_label: &str,
    ) -> Result<Option<u64>, Self::Error> {
        Ok(Some(0x5eed_0000_u64 + u64::from(step_index)))
    }

    fn input(
        &mut self,
        action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.inputs.push(format!("{action:?}"));
        if self.inputs.len() >= self.stop_at_input {
            return Err(Scripted::StoppedAtInput);
        }
        Ok(())
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        self.traces.push(format!("{trace:?}"));
        Ok(())
    }
}

fn run(task: &PreparedContainedTask, frames: Vec<Frame>, stop_at_input: usize) -> ScriptedRuntime {
    let mut runtime = ScriptedRuntime {
        frames: frames.into(),
        traces: Vec::new(),
        inputs: Vec::new(),
        stop_at_input,
    };
    let end = match task.run(&mut runtime) {
        Ok(outcome) => format!("ok {outcome:?}"),
        Err(ContainedTaskRunError::Task(error)) => {
            format!(
                "task_error code={} detail={:?}",
                error.code(),
                error.detail()
            )
        }
        Err(ContainedTaskRunError::NonfatalOperation(error)) => format!("operation {error:?}"),
        Err(ContainedTaskRunError::Boundary(error)) => format!("boundary {error:?}"),
    };
    runtime.traces.push(format!("END {end}"));
    runtime
}

fn frame(canvas: Vec<u8>) -> Frame {
    Frame::from_pixels(
        WIDTH as u32,
        HEIGHT as u32,
        canvas,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .unwrap_or_else(|error| panic!("frame: {error}"))
}

fn target<'a>(pack: &'a Value, id: &str) -> &'a Value {
    pack["targets"]
        .as_array()
        .expect("targets")
        .iter()
        .find(|target| target["id"] == id)
        .unwrap_or_else(|| panic!("target {id} missing"))
}

fn draw_target(canvas: &mut [u8], entries: &Entries, pack: &Value, id: &str) {
    let target = target(pack, id);
    let path = target["template_path"].as_str().expect("template_path");
    let crop = Scene::from_png(&entries[&format!("resources/{path}")])
        .unwrap_or_else(|error| panic!("{path}: {error}"));
    let x = target["region"]["x"].as_u64().expect("x") as usize;
    let y = target["region"]["y"].as_u64().expect("y") as usize;
    let width = crop.width() as usize;
    for row in 0..crop.height() as usize {
        let source = &crop.rgb8_pixels()[row * width * 3..(row + 1) * width * 3];
        let start = ((y + row) * WIDTH + x) * 3;
        canvas[start..start + width * 3].copy_from_slice(source);
    }
}

fn page(
    entries: &Entries,
    pack: &Value,
    ids: &[&str],
    pixels: &[((usize, usize), [u8; 3])],
) -> Frame {
    let mut canvas = vec![0_u8; WIDTH * HEIGHT * 3];
    for id in ids {
        draw_target(&mut canvas, entries, pack, id);
    }
    for ((x, y), color) in pixels {
        let start = (y * WIDTH + x) * 3;
        canvas[start..start + 3].copy_from_slice(color);
    }
    frame(canvas)
}

#[test]
fn one_off_308g4_control_packages_without_select() {
    let archive = unzip(&download_archive());
    let (bundle_name, bundle_bytes) = archive
        .iter()
        .find(|(path, _)| path.contains("/bundles/") && path.ends_with(".zip"))
        .unwrap_or_else(|| panic!("no bundle in the archive"));
    assert_eq!(digest_bytes(bundle_bytes), BUNDLE_SHA256, "{bundle_name}");
    let bundle = unzip(bundle_bytes);
    let index = json_entry(&bundle, "bundle.json").unwrap_or_else(|error| panic!("{error}"));
    let mut packs = Vec::new();
    for pack in index["packs"].as_array().expect("packs") {
        let path = pack["path"].as_str().expect("pack path");
        let bytes = bundle.get(path).unwrap_or_else(|| panic!("missing {path}"));
        assert_eq!(
            digest_bytes(bytes),
            pack["sha256"].as_str().expect("sha256"),
            "{path}"
        );
        packs.push((path.to_owned(), bytes.clone(), unzip(bytes)));
    }
    emit(&format!("bundle={BUNDLE_SHA256} packs={}", packs.len()));
    for (path, zip, entries) in &packs {
        // D: derived documents of the sources and admission of the sealed package.
        let derived = match convert(&source_entries(entries)) {
            Ok((outputs, canonical)) => format!(
                "pack_schema={} pages_schema={} navigation_schema={} index_schema={} primitives_schema={} pack={} pages={} navigation={} index={} primitives={} canonical_task={}",
                outputs.pack["schema_version"],
                outputs.pages["schema_version"],
                outputs.navigation["schema_version"],
                outputs.index["schema_version"],
                outputs.primitives["schema_version"],
                digest(&outputs.pack),
                digest(&outputs.pages),
                digest(&outputs.navigation),
                digest(&outputs.index),
                digest(&outputs.primitives),
                digest(&canonical)
            ),
            Err(error) => format!("convert=error {error}"),
        };
        emit(&format!("D pack={path} {derived}"));
        let task = match PreparedContainedTask::load("one-off.instance", zip, expected(zip)) {
            Ok(task) => task,
            Err(error) => {
                emit(&format!("D pack={path} admission=error {error:?}"));
                continue;
            }
        };
        emit(&format!("D pack={path} admission=ok"));
        // T: two black frames.
        let black = || frame(vec![0_u8; WIDTH * HEIGHT * 3]);
        let runtime = run(&task, vec![black(), black()], 1);
        let joined = runtime.traces.join("\n");
        emit(&format!(
            "T pack={path} traces={} trace_sha256={} {}",
            runtime.traces.len(),
            digest_bytes(joined.as_bytes()),
            runtime
                .traces
                .last()
                .map(String::as_str)
                .unwrap_or_default()
        ));
    }

    // S: the unmodified schedule package through its click path.
    let (_, zip, entries) = packs
        .iter()
        .find(|(_, _, entries)| {
            json_entry(entries, "control.json")
                .ok()
                .and_then(|control| control["entry_task_id"].as_str().map(str::to_owned))
                .as_deref()
                == Some(LIST_TASK)
        })
        .unwrap_or_else(|| panic!("no package for {LIST_TASK}"));
    let pack = entries
        .iter()
        .find(|(path, _)| path.ends_with(".pack.json"))
        .map(|(_, bytes)| serde_json::from_slice::<Value>(bytes).expect("pack"))
        .expect("pack.json");
    let task = PreparedContainedTask::load("one-off.instance", zip, expected(zip))
        .unwrap_or_else(|error| panic!("admission: {error:?}"));
    let frames = vec![
        page(
            entries,
            &pack,
            &["page/home", "ui/home_work_label", "ui/home_schedule_label"],
            &[],
        ),
        page(
            entries,
            &pack,
            &[
                "page/schedule_list",
                "ui/schedule_location_header",
                "ui/schedule_millennium_option",
            ],
            &[],
        ),
        page(
            entries,
            &pack,
            &[
                "page/schedule_area",
                "ui/schedule_millennium_area",
                "ui/schedule_all_button",
                "ui/return_home_icon",
            ],
            &[],
        ),
        page(
            entries,
            &pack,
            &[
                "page/schedule_all",
                "ui/schedule_all_close",
                "ui/schedule_course_tower",
            ],
            &[],
        ),
        page(
            entries,
            &pack,
            &["page/schedule_info", "ui/schedule_start"],
            &[((600, 576), [119, 222, 255])],
        ),
    ];
    let runtime = run(&task, frames, 5);
    for (index, trace) in runtime.traces.iter().enumerate() {
        emit(&format!("S trace[{index}] {trace}"));
    }
    for (index, input) in runtime.inputs.iter().enumerate() {
        emit(&format!("S input[{index}] {input}"));
    }
}
