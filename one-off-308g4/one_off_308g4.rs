// SPDX-License-Identifier: AGPL-3.0-only

//! One-off evidence for Workflow #308 G4 (model section 6, GC4), to be reverted; runs on the PR
//! head only. Every printed line starts with `G4|`.
//!
//! F: the course list task's source form, recovered from its sealed package in the published
//!    umbrella bundle (536f048a), gains a fixed_slots layout of three course slots on its course
//!    list page, a policy document and a select step in place of its course click.
//! P: the parser derives it: pack.json at 0.7, every other document at 0.6, no navigation edge
//!    and no page operation for the select step, its primitive and its sealed task.
//! M: four authoring mistakes are refused with pointers, by the parser and by admission.
//! R: as a content-directory package and as a `package build-task` ZIP, the task runs on a
//!    scripted runtime (fixed instant, empty fact snapshot) over frames composed of the
//!    package's own images at their declared rectangles: (a) the preferred course in slot 2,
//!    (b) no preferred course, (c) the confirmation frame from the other region state.

use actingcommand_contract::{
    InputAction, InstanceFactContext, InstanceFactSnapshot, LabError, PackageRef, TaskSemanticFact,
};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_execution_kernel::{
    ContainedTaskRunError, ContainedTaskRuntime, ContainedTaskRuntimeErrorClass,
    ContainedTaskTrace, ExternalExpectedSha256, InputFrameContext, ObservedFrame,
    PreparedContainedTask, SelectionState, SelectionStateRequest,
};
use actingcommand_lab::{
    PackageBuildTaskRequest, PackageEnvOptions, PackageSource, ResourceConvertRequest,
};
use actingcommand_pack_containment::source::{
    self, Bundle, OperationParser, ParseFiles, ParseOutputs, SourceFile, SourceRead,
};
use actingcommand_pack_containment::{Containment, Sha256Hash};
use actingcommand_recognition::Scene;
use actingcommand_resource_tooling::{
    AuthoringEnvironmentSnapshot, DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES, open_published_package,
    prepare_package_build_task, resource_convert,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

type Entries = BTreeMap<String, Vec<u8>>;

const ARCHIVE_URL: &str =
    "https://github.com/HS7097/ActingCommand/archive/536f048a3cac26ddbb0391895391f98967704cb0.zip";
const BUNDLE_SHA256: &str = "df524eac6290c4c0b435f3872a89e12dbe4f49e37555fa102bf96dd71b696af8";
const TASK: &str = "schedule_daily";
const SELECT_OPERATION: &str = "schedule_select_course";
const LIST_PAGE: &str = "schedule_all";
const LAYOUT_ID: &str = "layout/course_slots";
const POLICY_PATH: &str = "policies/course_slots.json";
const COURSE_TARGET: &str = "ui/schedule_course_tower";
const SLOT_X: [i64; 3] = [214, 504, 794];
const SLOT_Y: i64 = 204;
const NOW_UNIX_MS: u64 = 1_790_000_000_000;
const WIDTH: usize = 1280;
const HEIGHT: usize = 720;

fn emit(line: &str) {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "G4|{}", line.replace('\n', " | "))
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

fn json_entry(entries: &Entries, path: &str) -> Value {
    let bytes = entries
        .get(path)
        .unwrap_or_else(|| panic!("missing {path}"));
    serde_json::from_slice(bytes).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn digest_bytes(bytes: &[u8]) -> String {
    Sha256Hash::digest(bytes).to_string()
}

fn to_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap_or_else(|error| panic!("encode: {error}"))
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

fn task_path() -> String {
    format!("resources/operations/{TASK}/task.json")
}

fn is_derived(path: &str) -> bool {
    path == "resources/manifest.json"
        || path.ends_with(".pack.json")
        || path.ends_with(".pages.json")
        || path.ends_with(".navigation.json")
        || path.ends_with("operations.index.json")
        || path.ends_with("operations.primitives.json")
}

/// The source parse of one package's sources, as the content-directory loader parses them.
fn convert(entries: &Entries) -> Result<(ParseOutputs, Value), String> {
    let control = json_entry(entries, "control.json");
    let text = |key: &str| control[key].as_str().unwrap_or_default().to_owned();
    let (game, server, entry_task_id) = (text("game"), text("server"), text("entry_task_id"));
    let resources = json_entry(entries, "resources/operations/resources.json");
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
    let locale = entry.data["locale"].as_str().unwrap_or_default().to_owned();
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
                                SourceRead::BoundedBytes(limit)
                                | SourceRead::SelectionPolicy(limit)
                                    if bytes.len() as u64 > limit =>
                                {
                                    Err("parse input exceeds declared limit".to_owned())
                                }
                                _ => Ok(bytes.clone()),
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

/// The sealed package's sources: its canonical task with every drag click's `from_rect` /
/// `to_rect` returned to the source fields `from` / `to` (G2 one-off), without derived output.
fn source_form(package: &Entries) -> (Entries, Value) {
    let mut task = json_entry(package, &task_path());
    for operation in task["operations"].as_array_mut().expect("operations") {
        let Some(click) = operation.get_mut("click").and_then(Value::as_object_mut) else {
            continue;
        };
        if click.get("kind").and_then(Value::as_str) != Some("drag") {
            continue;
        }
        let from = click.remove("from_rect").expect("drag from_rect");
        let to = click.remove("to_rect").expect("drag to_rect");
        click.insert("from".to_owned(), from);
        click.insert("to".to_owned(), to);
    }
    let mut entries = package.clone();
    entries.retain(|path, _| !is_derived(path));
    entries.insert(task_path(), to_bytes(&task));
    (entries, task)
}

/// A probe rectangle inside the course card whose mean color is far from the empty slot's
/// black, and the mean, so a slot that shows a card reads as open.
struct Probe {
    x: i64,
    y: i64,
    width: i64,
    height: i64,
    mean: [u64; 3],
}

fn probe_of(course: &Scene) -> Probe {
    let width = course.width() as i64;
    let height = course.height() as i64;
    for y in [6_i64, 8, 4, 10] {
        for x in (4..width / 2 - 28).step_by(4) {
            let (probe_width, probe_height) = (16_i64, 10_i64.min(height - y));
            let mut sum = [0_u64; 3];
            for row in y..y + probe_height {
                for column in x..x + probe_width {
                    let start = ((row * width + column) * 3) as usize;
                    for (channel, total) in sum.iter_mut().enumerate() {
                        *total += u64::from(course.rgb8_pixels()[start + channel]);
                    }
                }
            }
            let area = (probe_width * probe_height) as u64;
            let mean = sum.map(|total| total / area);
            let distance =
                ((mean[0] * mean[0] + mean[1] * mean[1] + mean[2] * mean[2]) as f64).sqrt();
            if distance > 80.0 {
                return Probe {
                    x,
                    y,
                    width: probe_width,
                    height: probe_height,
                    mean,
                };
            }
        }
    }
    panic!("no probe rectangle in the course card is far from black");
}

fn policy_document(feature_type_preferred: &str, layout_id: &str, weight: i64) -> Vec<u8> {
    let preferred_term = if feature_type_preferred == "boolean" {
        json!({"term_id": "preferred", "value": {"source": "field", "field": "preferred"},
               "transform": {"kind": "lookup", "entries": [
                   {"key": {"type": "boolean", "value": true}, "value_milli": 1000},
                   {"key": {"type": "boolean", "value": false}, "value_milli": 0}]},
               "weight_milli": weight, "on_unknown": {"kind": "drop_candidate"}})
    } else {
        json!({"term_id": "preferred", "value": {"source": "field", "field": "preferred"},
               "transform": {"kind": "identity"},
               "weight_milli": weight, "on_unknown": {"kind": "drop_candidate"}})
    };
    serde_json::to_vec_pretty(&json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "course_slot_pick",
        "applies_to": {
            "candidate_layout_id": layout_id,
            "outcome_keys": {
                "selected": "course_selected",
                "empty": "course_none",
                "insufficient": "course_none_open",
                "ambiguous": "course_ambiguous",
                "unknown": "course_unknown"
            }
        },
        "fields": [
            {"name": "open", "value_type": {"type": "boolean"}},
            {"name": "preferred", "value_type": {"type": feature_type_preferred}}
        ],
        "facts": [],
        "gates": [{
            "gate_id": "open",
            "predicate": {"kind": "boolean_equals", "value": {"source": "field", "field": "open"}, "expected": true},
            "on_unknown": {"kind": "drop_candidate"}
        }],
        "scoring": [preferred_term],
        "selection": {"mode": "exactly_one", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    }))
    .expect("policy")
}

/// The course list task with three course slots, a policy and a select step in place of the
/// course click. `policy` and `declared_sha256` are the document and the hash the step seals.
fn select_form(
    task: &Value,
    course: &Scene,
    probe: &Probe,
    policy: &[u8],
    declared_sha256: &str,
) -> Value {
    let mut task = task.clone();
    let (card_width, card_height) = (course.width() as i64, course.height() as i64);
    for (slot, x) in SLOT_X.iter().enumerate() {
        task["verify_templates"]
            .as_array_mut()
            .expect("verify_templates")
            .push(json!({
                "id": format!("ui/course_slot_{slot}"),
                "template": "assets/schedule_course_tower.png",
                "region": {"mode": "rect", "rect": {"x": x, "y": SLOT_Y, "width": card_width, "height": card_height}},
                "threshold": 0.97
            }));
        task["color_probes"]
            .as_array_mut()
            .expect("color_probes")
            .push(json!({
                "id": format!("state/course_slot_{slot}_open"),
                "region": {"mode": "rect", "rect": {"x": x + probe.x, "y": SLOT_Y + probe.y, "width": probe.width, "height": probe.height}},
                "expected": probe.mean
            }));
    }
    task["candidate_layouts"] = json!([{
        "id": LAYOUT_ID,
        "page_id": LIST_PAGE,
        "kind": "fixed_slots",
        "features": [
            {"name": "open", "value": "passed"},
            {"name": "preferred", "value": "passed"}
        ],
        "slots": SLOT_X.iter().enumerate().map(|(slot, x)| json!({
            "rect": {"x": x, "y": SLOT_Y, "width": 176, "height": 90},
            "click": {"x": x + 10, "y": SLOT_Y + 10, "width": 156, "height": 60},
            "targets": {
                "open": format!("state/course_slot_{slot}_open"),
                "preferred": format!("ui/course_slot_{slot}")
            }
        })).collect::<Vec<_>>()
    }]);
    // A list page without the preferred course is still the list page.
    let required = task["page_rules"][LIST_PAGE]["required"]
        .as_array_mut()
        .expect("list page rule");
    required.retain(|target| target != COURSE_TARGET);
    let operation = task["operations"]
        .as_array_mut()
        .expect("operations")
        .iter_mut()
        .find(|operation| operation["id"] == SELECT_OPERATION)
        .expect("course operation");
    let object = operation.as_object_mut().expect("operation object");
    object.remove("click");
    object.insert(
        "select".to_owned(),
        json!({"layout_id": LAYOUT_ID, "policy": {"path": POLICY_PATH, "sha256": declared_sha256}}),
    );
    object.insert(
        "guard".to_owned(),
        json!({
            "page_id": LIST_PAGE,
            "target_id": "ui/schedule_all_close",
            "expected_rect": {"x": 1124, "y": 86, "width": 30, "height": 31},
            "verify_template": "assets/schedule_all_close.png"
        }),
    );
    let _ = policy;
    task
}

fn with_task(source: &Entries, task: &Value, policy: &[u8]) -> Entries {
    let mut entries = source.clone();
    entries.insert(task_path(), to_bytes(task));
    entries.insert(
        format!("resources/operations/{TASK}/{POLICY_PATH}"),
        policy.to_vec(),
    );
    entries
}

fn write_tree(root: &Path, entries: &Entries, strip: &str) {
    for (path, bytes) in entries {
        let Some(relative) = path.strip_prefix(strip) else {
            continue;
        };
        let target = root.join(relative);
        fs::create_dir_all(target.parent().expect("parent")).expect("create dir");
        fs::write(&target, bytes).expect("write file");
    }
}

/// The content-directory package of `entries`, admitted through the production loader.
fn content_directory(entries: &Entries, root: &Path) -> Result<PreparedContainedTask, String> {
    write_tree(root, entries, "");
    let deadline = Instant::now() + Duration::from_secs(60);
    let snapshot = Containment::new()
        .snapshot_content_directory(root, deadline)
        .map_err(|error| format!("snapshot {error:?}"))?;
    let reference = PackageRef::ContentDirectory(snapshot.reference.clone());
    emit(&format!(
        "R content_directory reference={}",
        serde_json::to_string(&reference).unwrap_or_default()
    ));
    PreparedContainedTask::load_path("one-off.instance", root, &reference, None, deadline)
        .map_err(|error| format!("{error:?}"))
}

/// `package build-task` of `entries`'s sources, admitted from the built ZIP's bytes.
fn build_task_zip(entries: &Entries, temp: &Path) -> Result<PreparedContainedTask, String> {
    let root = temp.join("resources");
    write_tree(&root, entries, "resources/");
    fs::create_dir_all(root.join("navigation")).expect("navigation dir");
    resource_convert(ResourceConvertRequest {
        repo: root.clone(),
        game: None,
        server: None,
        locale: None,
        maa_tasks_root: None,
        dry_run: false,
    })
    .map_err(|error| refusal("resource_convert", error))?;
    let out = temp.join("built.zip");
    let response = prepare_package_build_task(PackageBuildTaskRequest {
        source: PackageSource::Local(root.clone()),
        temporary_root: temp.join("build"),
        task_id: TASK.to_owned(),
        game: None,
        server: None,
        locale: None,
        package_id: None,
        execution_mode: Some("navigable_route".to_owned()),
        resolution: None,
        include_recovery: false,
        out: out.clone(),
        dry_run: false,
        max_buffered_payload_bytes: DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES,
        env: PackageEnvOptions::default(),
    })
    .map_err(|error| refusal("prepare_build", error))?
    .build(&AuthoringEnvironmentSnapshot::default())
    .map_err(|error| refusal("build", error))?;
    emit(&format!(
        "R zip build status={} task={}",
        response.status, response.task_id
    ));
    let bytes = open_published_package(&out)
        .and_then(|reader| reader.read_all())
        .map_err(|error| refusal("read_built", error))?;
    let built = unzip(&bytes);
    emit(&format!(
        "R zip sha256={} entries={} has_policy={} control={}",
        digest_bytes(&bytes),
        built.len(),
        built.contains_key(&format!("resources/operations/{TASK}/{POLICY_PATH}")),
        String::from_utf8_lossy(&built["control.json"]).replace('\n', "")
    ));
    let expected = ExternalExpectedSha256::parse_hex(&digest_bytes(&bytes))
        .map_err(|error| format!("{error:?}"))?;
    PreparedContainedTask::load("one-off.instance", &bytes, expected)
        .map_err(|error| format!("{error:?}"))
}

/// Serves composed frames with a fixed instant and an empty fact snapshot, records every
/// trace, and ends the run at the course detail page's input.
struct ScriptedRuntime {
    frames: VecDeque<Frame>,
    traces: Vec<ContainedTaskTrace>,
    inputs: Vec<(InputAction, bool)>,
    selection_requests: Vec<SelectionStateRequest>,
    stop_after: Option<String>,
    stop: bool,
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

    fn selection_state(
        &mut self,
        request: SelectionStateRequest,
    ) -> Result<SelectionState, Self::Error> {
        let context = InstanceFactContext {
            instance_id: "one-off.instance".to_owned(),
            server_id: request.server.clone(),
            game_id: request.game.clone(),
        };
        self.selection_requests.push(request);
        Ok(SelectionState::Snapshot {
            snapshot: InstanceFactSnapshot {
                snapshot_id: "snapshot:one-off:empty".to_owned(),
                ledger_position: 1,
                context,
                records: Vec::new(),
            },
            now_unix_ms: NOW_UNIX_MS,
        })
    }

    fn input(
        &mut self,
        action: InputAction,
        frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.inputs.push((action, frame.is_some()));
        if self.stop {
            return Err(Scripted::StoppedAtInput);
        }
        Ok(())
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        if let ContainedTaskTrace::EffectIntent {
            operation_label, ..
        } = &trace
            && self.stop_after.as_deref() == Some(operation_label.as_str())
        {
            self.stop = true;
        }
        self.traces.push(trace);
        Ok(())
    }
}

struct Run {
    end: String,
    record_json: Option<String>,
    record_valid_for_append: String,
    summary: Vec<String>,
}

fn run_case(task: &PreparedContainedTask, frames: Vec<Frame>) -> Run {
    let mut runtime = ScriptedRuntime {
        frames: frames.into(),
        traces: Vec::new(),
        inputs: Vec::new(),
        selection_requests: Vec::new(),
        stop_after: Some("schedule_start".to_owned()),
        stop: false,
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
    let mut summary = Vec::new();
    let mut record_json = None;
    let mut record_valid_for_append = "no record".to_owned();
    let mut chosen_click = None;
    for trace in &runtime.traces {
        match trace {
            ContainedTaskTrace::StepStarted {
                step_index,
                operation_label,
                from_page,
                ..
            } => summary.push(format!(
                "step_started {step_index} {operation_label} from={from_page}"
            )),
            ContainedTaskTrace::SelectionEvaluated {
                step_index,
                operation_label,
                selection,
            } => {
                record_valid_for_append = match selection.validate_for_append() {
                    Ok(()) => "ok".to_owned(),
                    Err(error) => format!("error {}:{}", error.code(), error.field()),
                };
                let fact = TaskSemanticFact::SelectionEvaluated {
                    step_index: *step_index,
                    operation_label: operation_label.clone(),
                    selection: selection.clone(),
                };
                record_json = Some(serde_json::to_string(&fact).expect("fact json"));
                if let Some(selected) = selection.selected.first() {
                    chosen_click = selection
                        .projection
                        .candidate(selected)
                        .map(|candidate| candidate.click);
                }
                summary.push(format!(
                    "selection_evaluated {step_index} {operation_label} outcome={} key={} selected={:?} evaluated_hash={} confirmation={} verdicts={}",
                    serde_json::to_string(&selection.outcome).unwrap_or_default(),
                    selection.outcome_key,
                    selection.selected,
                    selection.projection.candidate_set_sha256(),
                    serde_json::to_string(&selection.confirmation).unwrap_or_default(),
                    selection
                        .verdicts
                        .iter()
                        .map(|verdict| format!(
                            "{}:{:?}:{:?}:{:?}",
                            verdict.candidate_id, verdict.status, verdict.score_milli, verdict.rank
                        ))
                        .collect::<Vec<_>>()
                        .join(",")
                ));
                summary.push(format!(
                    "projection features {}",
                    selection
                        .projection
                        .candidates()
                        .iter()
                        .map(|candidate| format!(
                            "{}={}",
                            candidate.id,
                            serde_json::to_string(&candidate.features).unwrap_or_default()
                        ))
                        .collect::<Vec<_>>()
                        .join(" ")
                ));
            }
            ContainedTaskTrace::EffectIntent {
                step_index,
                operation_label,
                action,
                sampling,
                guard,
            } => {
                let inside = match (operation_label.as_str(), action, chosen_click) {
                    (SELECT_OPERATION, InputAction::Tap { x, y }, Some(click)) => format!(
                        " inside_chosen_click_rect={}",
                        *x >= click.x
                            && *x < click.x + click.width
                            && *y >= click.y
                            && *y < click.y + click.height
                    ),
                    _ => String::new(),
                };
                summary.push(format!(
                    "effect_intent {step_index} {operation_label} action={action:?} sampling={} guard={}{inside}",
                    serde_json::to_string(sampling).unwrap_or_default(),
                    serde_json::to_string(guard).unwrap_or_default()
                ));
            }
            ContainedTaskTrace::StepFinished {
                step_index,
                operation_label,
                page_label,
                ..
            } => summary.push(format!(
                "step_finished {step_index} {operation_label} page={page_label}"
            )),
            ContainedTaskTrace::RecognitionCompleted { page_label, .. } => {
                summary.push(format!("recognized {page_label:?}"));
            }
            _ => {}
        }
    }
    summary.push(format!(
        "inputs={} selection_requests={}",
        runtime
            .inputs
            .iter()
            .map(|(action, bound)| format!("{action:?} frame_bound={bound}"))
            .collect::<Vec<_>>()
            .join(";"),
        runtime
            .selection_requests
            .iter()
            .map(|request| format!("{request:?}"))
            .collect::<Vec<_>>()
            .join(";")
    ));
    Run {
        end,
        record_json,
        record_valid_for_append,
        summary,
    }
}

fn target<'a>(pack: &'a Value, id: &str) -> &'a Value {
    pack["targets"]
        .as_array()
        .expect("targets")
        .iter()
        .find(|target| target["id"] == id)
        .unwrap_or_else(|| panic!("target {id} missing"))
}

fn crop(entries: &Entries, pack: &Value, id: &str) -> Scene {
    let path = target(pack, id)["template_path"]
        .as_str()
        .expect("template_path");
    Scene::from_png(&entries[&format!("resources/{path}")])
        .unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn draw(canvas: &mut [u8], scene: &Scene, x: usize, y: usize, columns: usize) {
    let width = scene.width() as usize;
    for row in 0..scene.height() as usize {
        let source = &scene.rgb8_pixels()[row * width * 3..row * width * 3 + columns * 3];
        let start = ((y + row) * WIDTH + x) * 3;
        canvas[start..start + columns * 3].copy_from_slice(source);
    }
}

struct Canvas<'a> {
    entries: &'a Entries,
    pack: &'a Value,
}

impl Canvas<'_> {
    fn page(&self, ids: &[&str], extra: impl FnOnce(&mut [u8])) -> Frame {
        let mut canvas = vec![0_u8; WIDTH * HEIGHT * 3];
        for id in ids {
            let scene = crop(self.entries, self.pack, id);
            let region = &target(self.pack, id)["region"];
            draw(
                &mut canvas,
                &scene,
                region["x"].as_u64().expect("x") as usize,
                region["y"].as_u64().expect("y") as usize,
                scene.width() as usize,
            );
        }
        extra(&mut canvas);
        Frame::from_pixels(
            WIDTH as u32,
            HEIGHT as u32,
            canvas,
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .unwrap_or_else(|error| panic!("frame: {error}"))
    }
}

/// The course list in one region state: per slot, `None` (empty), `Some(false)` (a course card
/// that is not the preferred course: its left part) or `Some(true)` (the preferred course card).
fn list_state(
    canvas: &Canvas<'_>,
    course: &Scene,
    probe: &Probe,
    slots: [Option<bool>; 3],
) -> Frame {
    let partial = (probe.x + probe.width + 8) as usize;
    assert!(
        partial * 2 < course.width() as usize,
        "the partial course card would hold most of the card"
    );
    canvas.page(&["page/schedule_all", "ui/schedule_all_close"], |pixels| {
        for (slot, state) in slots.iter().enumerate() {
            match state {
                None => {}
                Some(preferred) => draw(
                    pixels,
                    course,
                    SLOT_X[slot] as usize,
                    SLOT_Y as usize,
                    if *preferred {
                        course.width() as usize
                    } else {
                        partial
                    },
                ),
            }
        }
    })
}

#[test]
fn one_off_308g4_select_step_end_to_end() {
    let archive = unzip(&download_archive());
    let (bundle_name, bundle_bytes) = archive
        .iter()
        .find(|(path, _)| path.contains("/bundles/") && path.ends_with(".zip"))
        .unwrap_or_else(|| panic!("no bundle in the archive"));
    assert_eq!(digest_bytes(bundle_bytes), BUNDLE_SHA256, "{bundle_name}");
    let bundle = unzip(bundle_bytes);
    let index: Value = serde_json::from_slice(&bundle["bundle.json"]).expect("bundle.json");
    let package = index["packs"]
        .as_array()
        .expect("packs")
        .iter()
        .map(|pack| {
            let path = pack["path"].as_str().expect("path");
            let bytes = &bundle[path];
            assert_eq!(
                digest_bytes(bytes),
                pack["sha256"].as_str().expect("sha256")
            );
            (path.to_owned(), unzip(bytes))
        })
        .find(|(_, entries)| json_entry(entries, "control.json")["entry_task_id"] == TASK)
        .unwrap_or_else(|| panic!("no package for {TASK}"));
    emit(&format!("bundle={BUNDLE_SHA256} package={}", package.0));
    let sealed = package.1;
    let sealed_pack = sealed
        .iter()
        .find(|(path, _)| path.ends_with(".pack.json"))
        .map(|(_, bytes)| serde_json::from_slice::<Value>(bytes).expect("pack"))
        .expect("pack.json");

    // F: the source form and the select form.
    let (source, source_task) = source_form(&sealed);
    let course = crop(&sealed, &sealed_pack, COURSE_TARGET);
    let probe = probe_of(&course);
    emit(&format!(
        "F course card {}x{} open probe x={} y={} {}x{} mean={:?}",
        course.width(),
        course.height(),
        probe.x,
        probe.y,
        probe.width,
        probe.height,
        probe.mean
    ));
    let policy = policy_document("boolean", LAYOUT_ID, 1000);
    let policy_sha256 = digest_bytes(&policy);
    emit(&format!(
        "F policy {POLICY_PATH} sha256={policy_sha256} document={}",
        String::from_utf8_lossy(&policy)
            .replace('\n', "")
            .replace("  ", "")
    ));
    let task = select_form(&source_task, &course, &probe, &policy, &policy_sha256);
    emit(&format!(
        "F select step {}",
        task["operations"]
            .as_array()
            .expect("operations")
            .iter()
            .find(|operation| operation["id"] == SELECT_OPERATION)
            .map(Value::to_string)
            .unwrap_or_default()
    ));
    emit(&format!(
        "F candidate_layouts {}",
        task["candidate_layouts"]
    ));
    let entries = with_task(&source, &task, &policy);

    // P: the parser's derived documents.
    match convert(&entries) {
        Err(error) => emit(&format!("P convert=error {error}")),
        Ok((outputs, canonical)) => {
            emit(&format!(
                "P pack_schema={} pages_schema={} navigation_schema={} index_schema={} primitives_schema={} pack_layouts={}",
                outputs.pack["schema_version"],
                outputs.pages["schema_version"],
                outputs.navigation["schema_version"],
                outputs.index["schema_version"],
                outputs.primitives["schema_version"],
                outputs.pack["candidate_layouts"]
            ));
            let ids = |key: &str| {
                outputs.navigation[key]
                    .as_array()
                    .map(|rows| {
                        rows.iter()
                            .filter_map(|row| row["id"].as_str())
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .unwrap_or_default()
            };
            emit(&format!(
                "P navigation edges=[{}] page_operations=[{}]",
                ids("navigation"),
                ids("page_operations")
            ));
            emit(&format!(
                "P primitive {}",
                outputs.primitives["primitives"]
                    .as_array()
                    .expect("primitives")
                    .iter()
                    .find(|primitive| primitive["id"] == SELECT_OPERATION)
                    .map(Value::to_string)
                    .unwrap_or_default()
            ));
            emit(&format!(
                "P sealed task operation {}",
                canonical["operations"]
                    .as_array()
                    .expect("operations")
                    .iter()
                    .find(|operation| operation["id"] == SELECT_OPERATION)
                    .map(Value::to_string)
                    .unwrap_or_default()
            ));
        }
    }

    // M: four authoring mistakes.
    let edited = policy_document("boolean", LAYOUT_ID, 900);
    let misspelled = policy_document("boolean", "layout/course_slot", 1000);
    let integer = policy_document("integer", LAYOUT_ID, 1000);
    let mut with_click = task.clone();
    with_click["operations"]
        .as_array_mut()
        .expect("operations")
        .iter_mut()
        .find(|operation| operation["id"] == SELECT_OPERATION)
        .expect("course operation")["click"] =
        json!({"kind": "target_center", "target_id": COURSE_TARGET});
    let mistakes = [
        (
            "policy edited but its sha256 not updated",
            with_task(&source, &task, &edited),
        ),
        (
            "applies_to layout id misspelled",
            with_task(
                &source,
                &select_form(
                    &source_task,
                    &course,
                    &probe,
                    &misspelled,
                    &digest_bytes(&misspelled),
                ),
                &misspelled,
            ),
        ),
        (
            "policy field declared integer for a passed feature",
            with_task(
                &source,
                &select_form(
                    &source_task,
                    &course,
                    &probe,
                    &integer,
                    &digest_bytes(&integer),
                ),
                &integer,
            ),
        ),
        (
            "select together with click",
            with_task(&source, &with_click, &policy),
        ),
    ];
    for (index, (case, mistaken)) in mistakes.iter().enumerate() {
        match convert(mistaken) {
            Ok(_) => emit(&format!("M{} {case}: parser NOT refused", index + 1)),
            Err(error) => emit(&format!("M{} {case}: parser refused {error}", index + 1)),
        }
        let temp = tempfile::TempDir::new().expect("temp");
        match content_directory(mistaken, &temp.path().join("package")) {
            Ok(_) => emit(&format!("M{} {case}: admission NOT refused", index + 1)),
            Err(error) => emit(&format!("M{} {case}: admission refused {error}", index + 1)),
        }
    }

    // R: the two package forms on the same scripted runs.
    let canvas = Canvas {
        entries: &sealed,
        pack: &sealed_pack,
    };
    let home = || {
        canvas.page(
            &["page/home", "ui/home_work_label", "ui/home_schedule_label"],
            |_| {},
        )
    };
    let list = || {
        canvas.page(
            &[
                "page/schedule_list",
                "ui/schedule_location_header",
                "ui/schedule_millennium_option",
            ],
            |_| {},
        )
    };
    let area = || {
        canvas.page(
            &[
                "page/schedule_area",
                "ui/schedule_millennium_area",
                "ui/schedule_all_button",
                "ui/return_home_icon",
            ],
            |_| {},
        )
    };
    let info = || {
        canvas.page(&["page/schedule_info", "ui/schedule_start"], |pixels| {
            let start = (576 * WIDTH + 600) * 3;
            pixels[start..start + 3].copy_from_slice(&[119, 222, 255]);
        })
    };
    // Region state A: course cards in slots 0 and 1, the preferred course in slot 2.
    let state_a = || {
        list_state(
            &canvas,
            &course,
            &probe,
            [Some(false), Some(false), Some(true)],
        )
    };
    // Region state B: slot 0 empty, course cards in slots 1 and 2, no preferred course.
    let state_b = || list_state(&canvas, &course, &probe, [None, Some(false), Some(false)]);

    let temp = tempfile::TempDir::new().expect("temp");
    let forms = [
        (
            "content_directory",
            content_directory(&entries, &temp.path().join("package")),
        ),
        (
            "zip",
            build_task_zip(&entries, &temp.path().join("build-task")),
        ),
    ];
    let mut records = BTreeMap::<String, Vec<Option<String>>>::new();
    for (form, prepared) in forms {
        let task = match prepared {
            Ok(task) => {
                emit(&format!("R {form} admission=ok"));
                task
            }
            Err(error) => {
                emit(&format!("R {form} admission=error {error}"));
                continue;
            }
        };
        for (case, f1, f2) in [
            ("a1", state_a(), state_a()),
            ("a2", state_a(), state_a()),
            ("b", state_b(), state_b()),
            ("c", state_a(), state_b()),
        ] {
            let run = run_case(&task, vec![home(), list(), area(), f1, f2, info()]);
            emit(&format!("R {form} case={case} end={}", run.end));
            for line in &run.summary {
                emit(&format!("R {form} case={case} {line}"));
            }
            emit(&format!(
                "R {form} case={case} record_validate_for_append={} task.selection_evaluated={}",
                run.record_valid_for_append,
                run.record_json.as_deref().unwrap_or("none")
            ));
            records
                .entry(case.to_owned())
                .or_default()
                .push(run.record_json);
        }
    }
    for (case, runs) in &records {
        emit(&format!(
            "R record bytes identical across forms case={case}: {} (forms={})",
            runs.windows(2).all(|pair| pair[0] == pair[1]),
            runs.len()
        ));
    }
    let a1 = records.get("a1");
    let a2 = records.get("a2");
    emit(&format!(
        "R two runs of case a give byte-identical task.selection_evaluated payloads: {}",
        a1.is_some() && a1 == a2
    ));
}
