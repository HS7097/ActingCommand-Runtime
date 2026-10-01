// SPDX-License-Identifier: AGPL-3.0-only

//! One-off evidence for the Workflow #308 G4 review fix (to be reverted): a fatal failure of
//! the runtime's confirmation capture after a `selected` decision. Every printed line starts
//! with `G4F|`.
//!
//! The package is the GC4 one-off's select form of the course list task (source recovered from
//! its sealed package in the published umbrella bundle 536f048a), as a content-directory
//! package and as a `package build-task` ZIP. A scripted runtime (fixed instant, empty fact
//! snapshot) serves frames composed of the package's own images:
//!   a1/b/c  the earlier GC4 cases, whose records must keep their earlier bytes;
//!   d       the confirmation capture fails with an error the runtime classifies fatal;
//!   e       as d, and the runtime also refuses the selection record;
//!   f       the confirmation capture fails with an error the runtime cannot classify;
//!   g       the confirmation capture fails nonfatally (frames exhausted), unchanged.

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
use actingcommand_pack_containment::{Containment, Sha256Hash};
use actingcommand_recognition::Scene;
use actingcommand_resource_tooling::{
    AuthoringEnvironmentSnapshot, DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES, open_published_package,
    prepare_package_build_task, resource_convert,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::process::Command;
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
/// The GC4 runs' record digests (runs 36904591550 and 36905741665, both package forms).
const EARLIER: [(&str, usize, &str); 3] = [
    (
        "a1",
        3718,
        "a13fb68c68c345db6efbf4b5c81a97015f42c4652c5f3f16a9a9d0acc2f620ac",
    ),
    (
        "b",
        3534,
        "7fc58d0df1329d8d4552cd4a9e0a482807656f9b8c8198551bd654fbec9a4713",
    ),
    (
        "c",
        3721,
        "c483af3d846bd8d025e643962f6bedf66c7f69e1bd97ab740284d37d8974bbdc",
    ),
];

fn emit(line: &str) {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "G4F|{}", line.replace('\n', " | "))
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

fn policy_document() -> Vec<u8> {
    serde_json::to_vec_pretty(&json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "course_slot_pick",
        "applies_to": {
            "candidate_layout_id": LAYOUT_ID,
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
            {"name": "preferred", "value_type": {"type": "boolean"}}
        ],
        "facts": [],
        "gates": [{
            "gate_id": "open",
            "predicate": {"kind": "boolean_equals", "value": {"source": "field", "field": "open"}, "expected": true},
            "on_unknown": {"kind": "drop_candidate"}
        }],
        "scoring": [{"term_id": "preferred", "value": {"source": "field", "field": "preferred"},
               "transform": {"kind": "lookup", "entries": [
                   {"key": {"type": "boolean", "value": true}, "value_milli": 1000},
                   {"key": {"type": "boolean", "value": false}, "value_milli": 0}]},
               "weight_milli": 1000, "on_unknown": {"kind": "drop_candidate"}}],
        "selection": {"mode": "exactly_one", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    }))
    .expect("policy")
}

/// The course list task with three course slots, a policy and a select step in place of the
/// course click (the GC4 one-off's select form).
fn select_form(task: &Value, course: &Scene, probe: &Probe, declared_sha256: &str) -> Value {
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
    prepare_package_build_task(PackageBuildTaskRequest {
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
    let bytes = open_published_package(&out)
        .and_then(|reader| reader.read_all())
        .map_err(|error| refusal("read_built", error))?;
    emit(&format!("R zip sha256={}", digest_bytes(&bytes)));
    let expected = ExternalExpectedSha256::parse_hex(&digest_bytes(&bytes))
        .map_err(|error| format!("{error:?}"))?;
    PreparedContainedTask::load("one-off.instance", &bytes, expected)
        .map_err(|error| format!("{error:?}"))
}

#[derive(Debug)]
enum Scripted {
    FramesExhausted,
    StoppedAtInput,
    /// A device capture failure the runtime classifies fatal (the host's real case).
    CaptureFatal,
    /// A capture failure the runtime cannot classify.
    CaptureUnknown,
    /// The runtime refuses to write the selection record.
    SelectionRecordRefused,
}

#[derive(Clone, Copy)]
enum CaptureFailure {
    Fatal,
    Unknown,
}

/// What a case does after the select attempt asked for its fact snapshot.
#[derive(Clone, Copy)]
struct Script {
    confirmation_capture: Option<CaptureFailure>,
    refuse_selection_record: bool,
}

const NORMAL: Script = Script {
    confirmation_capture: None,
    refuse_selection_record: false,
};

/// Serves composed frames with a fixed instant and an empty fact snapshot, records every
/// trace, and ends the run at the course detail page's input.
struct ScriptedRuntime {
    frames: VecDeque<Frame>,
    traces: Vec<ContainedTaskTrace>,
    refused: Vec<ContainedTaskTrace>,
    inputs: Vec<InputAction>,
    inputs_at_selection_record: Option<usize>,
    selection_requests: usize,
    script: Script,
    stop: bool,
}

impl ContainedTaskRuntime for ScriptedRuntime {
    type Error = Scripted;

    fn classify_error(error: &Self::Error) -> ContainedTaskRuntimeErrorClass {
        match error {
            Scripted::CaptureFatal | Scripted::SelectionRecordRefused => {
                ContainedTaskRuntimeErrorClass::Fatal
            }
            Scripted::CaptureUnknown => ContainedTaskRuntimeErrorClass::Unknown,
            Scripted::FramesExhausted | Scripted::StoppedAtInput => {
                ContainedTaskRuntimeErrorClass::Nonfatal
            }
        }
    }

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        if self.selection_requests > 0
            && let Some(failure) = self.script.confirmation_capture.take()
        {
            return Err(match failure {
                CaptureFailure::Fatal => Scripted::CaptureFatal,
                CaptureFailure::Unknown => Scripted::CaptureUnknown,
            });
        }
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
        self.selection_requests += 1;
        Ok(SelectionState::Snapshot {
            snapshot: InstanceFactSnapshot {
                snapshot_id: "snapshot:one-off:empty".to_owned(),
                ledger_position: 1,
                context: InstanceFactContext {
                    instance_id: "one-off.instance".to_owned(),
                    server_id: request.server,
                    game_id: request.game,
                },
                records: Vec::new(),
            },
            now_unix_ms: NOW_UNIX_MS,
        })
    }

    fn input(
        &mut self,
        action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.inputs.push(action);
        if self.stop {
            return Err(Scripted::StoppedAtInput);
        }
        Ok(())
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        if let ContainedTaskTrace::EffectIntent {
            operation_label, ..
        } = &trace
            && operation_label == "schedule_start"
        {
            self.stop = true;
        }
        if matches!(&trace, ContainedTaskTrace::SelectionEvaluated { .. }) {
            self.inputs_at_selection_record = Some(self.inputs.len());
            if self.script.refuse_selection_record {
                self.refused.push(trace);
                return Err(Scripted::SelectionRecordRefused);
            }
        }
        self.traces.push(trace);
        Ok(())
    }
}

fn record_line(trace: &ContainedTaskTrace) -> Option<(String, String)> {
    let ContainedTaskTrace::SelectionEvaluated {
        step_index,
        operation_label,
        selection,
        run_ending,
    } = trace
    else {
        return None;
    };
    let fact = TaskSemanticFact::SelectionEvaluated {
        step_index: *step_index,
        operation_label: operation_label.clone(),
        selection: selection.clone(),
    };
    let json = serde_json::to_string(&fact).expect("fact json");
    Some((
        format!(
            "step={step_index} {operation_label} run_ending={run_ending} outcome={} selected={:?} confirmation={} validate_for_append={} len={} sha256={}",
            serde_json::to_string(&selection.outcome).unwrap_or_default(),
            selection.selected,
            serde_json::to_string(&selection.confirmation).unwrap_or_default(),
            match selection.validate_for_append() {
                Ok(()) => "ok".to_owned(),
                Err(error) => format!("error {}:{}", error.code(), error.field()),
            },
            json.len(),
            digest_bytes(json.as_bytes())
        ),
        json,
    ))
}

fn run_case(
    form: &str,
    case: &str,
    task: &PreparedContainedTask,
    frames: Vec<Frame>,
    script: Script,
) {
    let mut runtime = ScriptedRuntime {
        frames: frames.into(),
        traces: Vec::new(),
        refused: Vec::new(),
        inputs: Vec::new(),
        inputs_at_selection_record: None,
        selection_requests: 0,
        script,
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
        Err(ContainedTaskRunError::NonfatalOperation(error)) => {
            format!("nonfatal_operation {error:?}")
        }
        Err(ContainedTaskRunError::Boundary(error)) => format!("boundary {error:?}"),
    };
    emit(&format!("R {form} case={case} end={end}"));
    let written = runtime
        .traces
        .iter()
        .filter_map(record_line)
        .collect::<Vec<_>>();
    emit(&format!(
        "R {form} case={case} selection_records_written={} handed_over_and_refused={}",
        written.len(),
        runtime.refused.len()
    ));
    for (line, _) in &written {
        emit(&format!("R {form} case={case} written {line}"));
    }
    for (line, _) in runtime.refused.iter().filter_map(record_line) {
        emit(&format!("R {form} case={case} refused {line}"));
    }
    if let Some((_, json)) = written.first() {
        if let Some((_, len, sha256)) = EARLIER.iter().find(|(earlier, ..)| *earlier == case) {
            emit(&format!(
                "R {form} case={case} record equals the earlier GC4 bytes (len {len}, sha256 {sha256}): {}",
                json.len() == *len && digest_bytes(json.as_bytes()) == *sha256
            ));
        }
        if case != "a1" {
            emit(&format!("R {form} case={case} task.selection_evaluated={json}"));
        }
    }
    let select_intents = runtime
        .traces
        .iter()
        .filter(|trace| {
            matches!(trace, ContainedTaskTrace::EffectIntent { operation_label, .. } if operation_label == SELECT_OPERATION)
        })
        .count();
    emit(&format!(
        "R {form} case={case} inputs_total={} inputs_after_selection_record={} select_step_effect_intents={select_intents} frames_left={}",
        runtime.inputs.len(),
        runtime
            .inputs_at_selection_record
            .map_or_else(|| "no record".to_owned(), |at| (runtime.inputs.len() - at).to_string()),
        runtime.frames.len()
    ));
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
fn one_off_308g4_fatal_confirmation_capture() {
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

    let (source, source_task) = source_form(&sealed);
    let course = crop(&sealed, &sealed_pack, COURSE_TARGET);
    let probe = probe_of(&course);
    let policy = policy_document();
    let policy_sha256 = digest_bytes(&policy);
    emit(&format!("F policy {POLICY_PATH} sha256={policy_sha256}"));
    let task = select_form(&source_task, &course, &probe, &policy_sha256);
    let entries = with_task(&source, &task, &policy);

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
    let state_a = || {
        list_state(
            &canvas,
            &course,
            &probe,
            [Some(false), Some(false), Some(true)],
        )
    };
    let state_b = || list_state(&canvas, &course, &probe, [None, Some(false), Some(false)]);
    let fatal = Script {
        confirmation_capture: Some(CaptureFailure::Fatal),
        refuse_selection_record: false,
    };
    let fatal_refused = Script {
        confirmation_capture: Some(CaptureFailure::Fatal),
        refuse_selection_record: true,
    };
    let unknown = Script {
        confirmation_capture: Some(CaptureFailure::Unknown),
        refuse_selection_record: false,
    };

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
        let cases: [(&str, Vec<Frame>, Script); 7] = [
            (
                "a1",
                vec![home(), list(), area(), state_a(), state_a(), info()],
                NORMAL,
            ),
            (
                "b",
                vec![home(), list(), area(), state_b(), state_b(), info()],
                NORMAL,
            ),
            (
                "c",
                vec![home(), list(), area(), state_a(), state_b(), info()],
                NORMAL,
            ),
            (
                "d",
                vec![home(), list(), area(), state_a(), state_a(), info()],
                fatal,
            ),
            (
                "e",
                vec![home(), list(), area(), state_a(), state_a(), info()],
                fatal_refused,
            ),
            (
                "f",
                vec![home(), list(), area(), state_a(), state_a(), info()],
                unknown,
            ),
            ("g", vec![home(), list(), area(), state_a()], NORMAL),
        ];
        for (case, frames, script) in cases {
            run_case(form, case, &task, frames, script);
        }
    }
}
