// SPDX-License-Identifier: AGPL-3.0-only

// #463 R1 (contracts/lab-recording.md, "Selecting in a list"; register §1.381; brief
// rd-1115/brief-463-r1-record-select.md; design rd-1114/463-l3l5-design.md, sections C.1 and C.2 and
// the coordinator's rulings): a recording step that chooses in a list of fixed slots by the name read
// in each slot. A request's `select` declares two slots, their name read, the names `item_a`
// (`ALPHA`) and `item_b` (`BRAVO`), the wanted value `list_item` with its default and the name the
// demonstration presses; the step's click is the slot the pressed name selects on the primary frame.
// The mark-time self-test is the kernel's dry run of the step as record stop generates it; record
// stop generates the layout, the slot targets, the select operation with `on_empty`, the policy
// document, the `on_empty` mapping and `lab.selects`, and the daemon's admission takes the package.
//
// Frames are synthetic: 96x64 pixels of flat gray. The scripted OCR provider reads one block per
// marker pixel inside the requested region, its text chosen by the marker's gray value, at
// confidence 0.9; a region without a marker reads nothing. Slot 0 is (8, 8, 32, 32) with its name
// read in (10, 28, 28, 8), slot 1 (48, 8, 32, 32) with (50, 28, 28, 8); a name's marker stands at
// (12 + 40k, 30). The color mark `state/page` on a flat area recognizes the list page and guards
// the select. No device is used; the tests read no clock and never sleep. Every expected value
// comes from the brief's and the design's lines and the values above, never from the code.
//
// Fix round 1 (brief rd-1115/brief-1729-fix1.md): each frame's decisions by their kind; the wanted
// name and the task's `_done` key have words of their own; a select on an application step is
// refused, never dropped; a select added after stop keeps the tap it took over and clear_select
// gives it back; another step's page passing on the select's frame is refused at mark time; the
// probe holds the select step record stop generates; a recording without a select is compared
// with constants.

use crate::codes::{LabCode, SelectProblem};
use crate::recording::crosscheck::admit;
use crate::recording::generate::{render, select_probe_plan};
use crate::recording::model::LabRecording;
use crate::recording::store::{hex_sha256, old_record_path, recording_path};
use crate::{
    AttachFrameRequest, ClickEffect, ClickPlan, CommitClickRequest, LAB_RECORD_MARK_SCHEMA, LabOcr,
    MarkOutcome, MarkRequest, PlanClickRequest, RecordRect, RecordStartOptions, RecordStepPorts,
    RecordStepRequest, RecordStopOptions, RecordingLock, record_attach_frame, record_commit_click,
    record_mark, record_plan_click, record_plan_step, record_start, record_stop,
};
use actingcommand_contract::{InputAction, LabError, LabResult};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_execution_kernel::{OfflineDecision, simulate_contained_task_at};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, PackRect, VisionProvider, VisionProviderError, VisionProviderErrorCode,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_r1";
const TASK_ID: &str = "neutral_task";
const WIDTH: u32 = 96;
const HEIGHT: u32 = 64;
const BACKGROUND: u8 = 60;
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PAGE: &str = "state/page";
const NOT_FOUND: &str = "neutral_task_not_found";
const POLICY: &str = "resources/operations/neutral_task/policies/step_01_select.json";
const NEXT_SEED: u64 = 0x0463_0001;

// The gray value of a marker pixel and the name the provider reads from it.
const ALPHA: u8 = 211;
const BRAVO: u8 = 212;
const MARKERS: [(u8, &str); 2] = [(ALPHA, "ALPHA"), (BRAVO, "BRAVO")];

const fn rect(x: i32, y: i32, width: i32, height: i32) -> RecordRect {
    RecordRect {
        x,
        y,
        width,
        height,
    }
}

// The slots' rectangles and the regions their names are read in.
const SLOTS: [RecordRect; 2] = [rect(8, 8, 32, 32), rect(48, 8, 32, 32)];
const NAMES: [RecordRect; 2] = [rect(10, 28, 28, 8), rect(50, 28, 28, 8)];

// Reads one block per marker pixel of the requested region at confidence 0.9; a region without a
// marker reads nothing, with no confidence.
#[derive(Debug)]
struct NamesOcr;

impl VisionProvider for NamesOcr {
    fn require_ocr_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Ok(())
    }

    fn require_nn_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }

    fn read_text(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        let region = request.region;
        let (right, bottom) = (region.x + region.width, region.y + region.height);
        let mut blocks = Vec::new();
        for y in region.y..bottom {
            for x in region.x..right {
                let index = (y as usize * request.frame.width as usize + x as usize) * 3;
                let pixel = request.frame.rgb8_pixels.get(index).copied();
                if let Some((_, name)) = MARKERS.iter().find(|(value, _)| Some(*value) == pixel) {
                    blocks.push(OcrProviderTextBlock {
                        text: (*name).to_owned(),
                        rect: PackRect {
                            x,
                            y,
                            width: 1,
                            height: 1,
                        },
                        confidence: Some(0.9),
                    });
                }
            }
        }
        let text = blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            confidence: (!blocks.is_empty()).then_some(0.9),
            text,
            blocks,
            detection: None,
        })
    }

    fn classify(
        &self,
        _request: NnProviderRequest<'_>,
    ) -> Result<NnProviderResult, VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }
}

// A deterministic generator of neutral gray noise below every marker value.
struct Noise(u64);

impl Noise {
    fn between(&mut self, low: u8, high: u8) -> u8 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        low + ((self.0 >> 33) % (u64::from(high - low) + 1)) as u8
    }
}

// The PNG of a gray screen, row-major values drawn with equal red, green and blue.
fn png(values: &[u8]) -> Vec<u8> {
    let pixels = values
        .iter()
        .flat_map(|value| [*value; 3])
        .collect::<Vec<_>>();
    Frame::from_pixels(
        WIDTH,
        HEIGHT,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
    .encode_png_fast()
    .expect("fixture PNG")
}

// The list showing in slot `k` the marker given for it (`None`: no name).
fn list(names: [Option<u8>; 2]) -> Vec<u8> {
    list_on(names, BACKGROUND)
}

// The list of `names` whose page area (2, 50, 4, 4), where the page marks stand, has the gray
// value `page`.
fn list_on(names: [Option<u8>; 2], page: u8) -> Vec<u8> {
    let mut values = vec![BACKGROUND; (WIDTH * HEIGHT) as usize];
    for row in 50..54 {
        for column in 2..6 {
            values[(row * WIDTH + column) as usize] = page;
        }
    }
    for (slot, marker) in (0_u32..).zip(names) {
        if let Some(marker) = marker {
            values[(30 * WIDTH + 12 + 40 * slot) as usize] = marker;
        }
    }
    png(&values)
}

// Fix 1: one frame's decisions as recording.json, the mark's answer and lab.selects report them:
// the pressed name's and the default's, each `selected` with its slot or (`None`) `empty`.
fn decisions(frame_id: &str, pressed: Option<u32>, default: Option<u32>) -> Value {
    let kind = |slot: Option<u32>| match slot {
        Some(slot) => json!({"result": "selected", "slot": slot}),
        None => json!({"result": "empty"}),
    };
    json!({"frame_id": frame_id, "pressed": kind(pressed), "default": kind(default)})
}

// The next screen: a noise patch at (40, 46), with no marker value.
fn next_screen() -> Vec<u8> {
    let mut noise = Noise(NEXT_SEED);
    let mut values = vec![BACKGROUND; (WIDTH * HEIGHT) as usize];
    for row in 0..12 {
        for column in 0..12 {
            values[((46 + row) * WIDTH + 40 + column) as usize] = noise.between(1, 199);
        }
    }
    png(&values)
}

// The page mark of the list: a color mark on a flat area no marker reaches.
fn page_mark() -> Value {
    json!({"id": PAGE, "family": "color", "region": rect(2, 50, 4, 4)})
}

// The select over the two slots with the wanted value's `default` and, when given, `pressed`.
fn select(pressed: Option<&str>, default: &str) -> Value {
    let mut select = json!({
        "slots": [{"rect": SLOTS[0], "name": NAMES[0]}, {"rect": SLOTS[1], "name": NAMES[1]}],
        "read": {"languages": ["en"], "timeout_ms": 1000, "minimum_confidence_milli": 800,
                 "model_ref": "neutral-ocr", "model_sha256": MODEL_SHA256},
        "names": [{"id": "item_a", "aliases": ["ALPHA"]}, {"id": "item_b", "aliases": ["BRAVO"]}],
        "match": {"max_distance": 1, "minimum_margin": 1},
        "wanted": {"name": "list_item", "default": default}
    });
    if let Some(pressed) = pressed {
        select["pressed"] = json!(pressed);
    }
    select
}

fn request(mut value: Value) -> MarkRequest {
    value["schema_version"] = json!(LAB_RECORD_MARK_SCHEMA);
    serde_json::from_value(value).expect("a record mark request")
}

fn inside(point: (i32, i32), area: RecordRect) -> bool {
    point.0 >= area.x
        && point.1 >= area.y
        && point.0 < area.x + area.width
        && point.1 < area.y + area.height
}

fn performed() -> CommitClickRequest {
    CommitClickRequest {
        effect: ClickEffect::Performed,
        has_failure: false,
        carrier_package: None,
        req_id: None,
        correlation_id: None,
        action_id: None,
        lease_id: None,
        failure: None,
        before: None,
        after: None,
    }
}

// A state directory with an active session of `INSTANCE` and its Lab recording, before any step,
// with the scripted OCR provider (`ocr`) and without an OCR model (`none`). The lock is dropped
// before the directory.
struct Recording {
    lock: RecordingLock,
    ocr: LabOcr,
    none: LabOcr,
    temp: TempDir,
}

impl Recording {
    fn new() -> Self {
        let temp = TempDir::new().expect("temporary directory");
        let state = temp.path().join("state");
        fs::create_dir_all(&state).expect("state directory");
        fs::write(
            old_record_path(&state, INSTANCE),
            serde_json::to_vec(&json!({
                "record_id": RECORD_ID,
                "task_id": TASK_ID,
                "instance": INSTANCE,
                "status": "active",
                "started_at_unix_ms": 1_700_000_000_000_u64
            }))
            .expect("session JSON"),
        )
        .expect("session file");
        let lock =
            RecordingLock::acquire(&state, INSTANCE, "gate_r1", "Lab-CLI").expect("recording lock");
        record_start(
            &lock,
            &RecordStartOptions {
                game: Some("neutral".to_owned()),
                server: Some("test".to_owned()),
                locale: Some("en-US".to_owned()),
                ..RecordStartOptions::default()
            },
        )
        .expect("record start");
        Self {
            lock,
            ocr: LabOcr::with_provider(Arc::new(NamesOcr)),
            none: LabOcr::from_vision_root(&temp.path().join("no-vision")),
            temp,
        }
    }

    // Writes `bytes` as the frame file `name` and returns its path.
    fn frame(&self, name: &str, bytes: &[u8]) -> String {
        let path = self.temp.path().join(format!("{name}.png"));
        fs::write(&path, bytes).expect("frame file");
        path.display().to_string()
    }

    fn mark_with(&self, value: Value, ocr: &LabOcr) -> Result<MarkOutcome, LabError> {
        record_mark(&self.lock, &request(value), false, ocr)
    }

    fn mark(&self, value: Value) -> Result<MarkOutcome, LabError> {
        self.mark_with(value, &self.ocr)
    }

    // Step 1 on `primary` with its samples: the page mark and `effect` (a `select` or a `click`).
    fn step_one(&self, primary: &[u8], samples: &[&[u8]], effect: (&str, Value)) -> Value {
        let mut value = json!({
            "frame": self.frame("primary", primary),
            "samples": samples
                .iter()
                .enumerate()
                .map(|(index, sample)| self.frame(&format!("sample_{index}"), sample))
                .collect::<Vec<_>>(),
            "add": [page_mark()]
        });
        value[effect.0] = effect.1;
        value
    }

    // The final step: the next screen with its own template mark.
    fn mark_next_step(&self, ocr: &LabOcr) {
        let frame = self.frame("next", &next_screen());
        self.mark_with(
            json!({
                "frame": frame,
                "add": [{"id": "ui/next", "family": "template", "region": rect(40, 46, 12, 12),
                         "search": rect(40, 46, 12, 12)}]
            }),
            ocr,
        )
        .unwrap_or_else(|error| panic!("record mark of the next step: {}", error.code));
    }

    // The click of the open step planned and committed as performed, which closes the step;
    // returns the pressed point.
    fn press_open_step(&self) -> (i32, i32) {
        let plan = record_plan_click(&self.lock, &PlanClickRequest::default())
            .unwrap_or_else(|error| panic!("plan click: {}", error.code));
        record_commit_click(&self.lock, &plan, &performed())
            .unwrap_or_else(|error| panic!("commit click: {}", error.code));
        (plan.point.x, plan.point.y)
    }

    // `record stop` with `ocr`: the `lab` output with its status.
    fn stop_with(&self, ocr: &LabOcr) -> Value {
        let outcome =
            record_stop(&self.lock, &RecordStopOptions::default(), ocr).unwrap_or_else(|error| {
                panic!(
                    "record stop: {}: {} {:?}",
                    error.code, error.message, error.details
                )
            });
        let mut lab = serde_json::to_value(&outcome.lab).expect("lab JSON");
        lab["lab_status"] = json!(outcome.lab_status);
        lab
    }

    // `record stop`, then the session file stopped as the command leaves it, so the stopped
    // recording takes `record mark` ("Marking after stop").
    fn stop_session(&self) -> Value {
        let lab = self.stop_with(&self.ocr);
        let path = old_record_path(&self.temp.path().join("state"), INSTANCE);
        let mut session: Value =
            serde_json::from_slice(&fs::read(&path).expect("session file")).expect("session");
        session["status"] = json!("stopped");
        fs::write(&path, serde_json::to_vec(&session).expect("session JSON"))
            .expect("session file");
        lab
    }

    // The files of the package `record stop` wrote.
    fn package(&self, lab: &Value) -> BTreeMap<String, Vec<u8>> {
        let bytes = fs::read(lab["path"].as_str().expect("package path")).expect("package");
        expand_content_container(&bytes, ContentContainer::Zip, ContainmentLimits::default())
            .expect("package container")
    }

    fn recording_bytes(&self) -> Vec<u8> {
        fs::read(recording_path(&self.temp.path().join("state"), RECORD_ID))
            .expect("recording file")
    }

    fn recording(&self) -> Value {
        serde_json::from_slice(&self.recording_bytes()).expect("recording")
    }
}

fn task(entries: &BTreeMap<String, Vec<u8>>) -> Value {
    let bytes = entries
        .get(&format!("resources/operations/{TASK_ID}/task.json"))
        .expect("task.json");
    serde_json::from_slice(bytes).expect("task JSON")
}

fn document(entries: &BTreeMap<String, Vec<u8>>, path: &str) -> Value {
    serde_json::from_slice(entries.get(path).expect("the document")).expect("document JSON")
}

// The `select_problem` of a `record_select_invalid` refusal, its outcome carrying the same word.
fn select_problem(error: LabError) -> String {
    assert_eq!(
        error.code,
        LabCode::RecordSelectInvalid.as_str(),
        "{}",
        error.message
    );
    let details = error.details.expect("details");
    assert_eq!(
        details["outcome"]["values"]["select_problem"], details["select_problem"],
        "{details}"
    );
    details["select_problem"]
        .as_str()
        .expect("a word")
        .to_owned()
}

// The refused select of `record_mark_rejected` with `reason`.
fn select_rejection(error: LabError, reason: &str) -> Value {
    assert_eq!(error.code, "record_mark_rejected", "{}", error.message);
    let details = error.details.expect("details");
    details["marks"]
        .as_array()
        .and_then(|marks| {
            marks
                .iter()
                .find(|mark| mark["id"] == "select" && mark["reason"] == reason)
        })
        .cloned()
        .unwrap_or_else(|| panic!("no select {reason} rejection in {details}"))
}

// Items 1, 2, 4 and 5 of the brief: a marked select is accepted, its click the slot the pressed
// name selects on the primary frame, its self-test passed with the default's decision reported;
// record stop generates the layout, the slot targets, the select operation with on_empty, the policy
// document's one gate, the on_empty mapping and lab.selects, and the daemon's admission takes the
// package, which taps the slot the wanted value's default selects.
#[test]
fn r1_a_marked_select_is_accepted_generated_and_admitted() {
    let recording = Recording::new();
    let primary = list([Some(ALPHA), Some(BRAVO)]);
    let marked = recording
        .mark(recording.step_one(&primary, &[], ("select", select(Some("item_a"), "item_b"))))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    let click = serde_json::to_value(&marked.click).expect("click JSON");
    assert_eq!(click["source"], "select", "{click}");
    assert_eq!(click["rect"], json!(SLOTS[0]), "{click}");
    let marked_select = serde_json::to_value(&marked.select).expect("select JSON");
    assert_eq!(
        marked_select["self_test"]["status"], "passed",
        "{marked_select}"
    );
    assert_eq!(
        marked_select["self_test"]["frames"],
        json!([decisions("f0001", Some(0), Some(1))]),
        "{marked_select}"
    );
    assert_eq!(
        recording.recording()["steps"][0]["select"]["declaration"],
        select(Some("item_a"), "item_b")
    );
    let point = recording.press_open_step();
    assert!(inside(point, SLOTS[0]), "{point:?}");
    recording.mark_next_step(&recording.ocr);

    let lab = recording.stop_with(&recording.ocr);

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let entries = recording.package(&lab);
    let task = task(&entries);
    let operation = &task["operations"][0];
    assert_eq!(operation["id"], "step_01_select", "{operation}");
    assert_eq!(
        operation["select"],
        json!({"layout_id": "layout/step_01",
               "policy": {"path": "policies/step_01_select.json"},
               "on_empty": {"outcome_key": NOT_FOUND}}),
        "{operation}"
    );
    assert_eq!(operation.get("click"), None, "{operation}");
    assert_eq!(operation["guard"]["color_probe"], PAGE, "{operation}");
    let layout = &task["candidate_layouts"][0];
    assert_eq!(layout["id"], "layout/step_01", "{layout}");
    assert_eq!(layout["kind"], "fixed_slots", "{layout}");
    assert_eq!(
        layout["features"][0]["identity"]["entries"],
        json!([{"id": "item_a", "aliases": ["ALPHA"]}, {"id": "item_b", "aliases": ["BRAVO"]}]),
        "{layout}"
    );
    assert_eq!(
        layout["slots"][1],
        json!({"rect": SLOTS[1], "click": SLOTS[1],
               "targets": {"name": "select/step_01/slot_01"}}),
        "{layout}"
    );
    let targets = task["ocr_targets"].as_array().expect("ocr targets");
    assert_eq!(targets.len(), 2, "{task}");
    assert_eq!(targets[0]["id"], "select/step_01/slot_00", "{task}");
    assert_eq!(targets[0]["expected"], json!(["ALPHA", "BRAVO"]), "{task}");
    assert_eq!(
        task["scheduling_outcome"]["mappings"],
        json!([
            {"outcome_key": "neutral_task_done", "effect": "no_designated_effect",
             "terminal_pages": ["step_02"]},
            {"outcome_key": NOT_FOUND, "effect": "no_designated_effect",
             "terminal_pages": ["step_01"]}
        ]),
        "{task}"
    );
    let policy = document(&entries, POLICY);
    assert_eq!(
        policy["gates"],
        json!([{"gate_id": "wanted.list_item",
                "predicate": {"kind": "string_in", "value": {"source": "field", "field": "name"},
                              "allowed": ["item_b"]},
                "on_unknown": {"kind": "drop_candidate"}}]),
        "{policy}"
    );
    assert_eq!(
        policy["applies_to"]["outcome_keys"]["empty"], NOT_FOUND,
        "{policy}"
    );
    assert_eq!(
        policy["selection"],
        json!({"mode": "none_allowed", "required_count": 1})
    );
    assert_eq!(document(&entries, "control.json").get("variables"), None);
    assert_eq!(
        lab["selects"],
        json!([{"step": 1, "record_index": 1, "op": "step_01_select", "layout": "layout/step_01",
                "wanted": {"name": "list_item", "default": "item_b"}, "pressed": "item_a",
                "on_empty_key": NOT_FOUND,
                "frames": [decisions("f0001", Some(0), Some(1))]}]),
        "{lab}"
    );
    assert_eq!(lab["first_decision"]["status"], "would_click", "{lab}");
    assert_eq!(lab["first_decision"]["slot"], 1, "{lab}");
    let prepared =
        admit(&entries, &recording.ocr).unwrap_or_else(|error| panic!("admission: {}", error.code));
    let frame = Frame::from_png(primary, CaptureBackendName::AdbScreencap).expect("frame");
    let decision = simulate_contained_task_at(&prepared, vec![frame], 1)
        .expect("simulation")
        .decision;
    match decision {
        OfflineDecision::WouldClick {
            operation_label,
            action: InputAction::Tap { x, y },
            ..
        } => {
            assert_eq!(operation_label, "step_01_select");
            assert!(inside((x, y), SLOTS[1]), "{x},{y}");
        }
        other => panic!("not a tap: {other:?}"),
    }
}

// Item 3 of the brief: each refusal of the declaration with its word; nothing is written.
#[test]
fn r1_each_select_rule_is_refused_with_its_word() {
    let recording = Recording::new();
    let primary = list([Some(ALPHA), Some(BRAVO)]);
    let base = || recording.step_one(&primary, &[], ("select", select(Some("item_a"), "item_b")));
    let with = |member: &str, value: Value| {
        let mut changed = base();
        changed[member] = value;
        changed
    };
    let selecting = |change: &dyn Fn(&mut Value)| {
        let mut changed = base();
        change(&mut changed["select"]);
        changed
    };
    let cases = [
        (
            with("click", json!({"follow": PAGE})),
            SelectProblem::WithFollow,
        ),
        (
            with(
                "click",
                json!({"region": SLOTS[0], "drag_to": SLOTS[1], "duration_ms": 300}),
            ),
            SelectProblem::WithGesture,
        ),
        (
            with("click", json!({"region": SLOTS[0]})),
            SelectProblem::WithClick,
        ),
        (
            with("retry", json!({"max_attempts": 2, "interval_ms": 1000})),
            SelectProblem::WithRetry,
        ),
        (with("optional", json!(true)), SelectProblem::OnOptionalStep),
        (
            with("application", json!({"action": "restart"})),
            SelectProblem::WithApplication,
        ),
        (
            selecting(&|select| select["wanted"]["default"] = json!("item_c")),
            SelectProblem::DefaultNotInNames,
        ),
        (
            selecting(&|select| select["names"][0]["id"] = json!("Item_A")),
            SelectProblem::NameIdInvalid,
        ),
        // Fix 1: the wanted name has a word of its own.
        (
            selecting(&|select| select["wanted"]["name"] = json!("list-item")),
            SelectProblem::WantedNameInvalid,
        ),
        (
            selecting(&|select| select["names"][1]["id"] = json!("item_a")),
            SelectProblem::NameDuplicate,
        ),
        (
            selecting(&|select| select["pressed"] = json!("item_c")),
            SelectProblem::PressedNotInNames,
        ),
        (
            selecting(&|select| select["on_empty_key"] = json!("Sold Out")),
            SelectProblem::OnEmptyKeyInvalid,
        ),
        // Fix 1: the task's own `_done` key has a word of its own; another select's key is
        // on_empty_key_taken (the next test).
        (
            selecting(&|select| select["on_empty_key"] = json!("neutral_task_done")),
            SelectProblem::OnEmptyKeyReserved,
        ),
        (
            selecting(&|select| select["slots"][1]["rect"] = json!(rect(80, 8, 32, 32))),
            SelectProblem::SlotOutsideFrame,
        ),
        (
            selecting(&|select| select["read"]["timeout_ms"] = json!(600_000)),
            SelectProblem::KernelRefused,
        ),
    ];
    for (value, problem) in cases {
        let error = recording
            .mark(value)
            .expect_err("the declaration cannot hold");
        assert_eq!(select_problem(error), problem.as_str());
        assert_eq!(recording.recording()["steps"], json!([]), "nothing written");
    }
    // A page with only an OCR mark gives no guard.
    let mut ocr_page = base();
    ocr_page["add"] = json!([{
        "id": "text/title", "family": "ocr", "region": NAMES[0], "languages": ["en"],
        "timeout_ms": 1000, "match_mode": "exact", "expected": ["ALPHA"],
        "case_sensitive": false, "minimum_confidence": 0.8, "model_ref": "neutral-ocr",
        "model_sha256": MODEL_SHA256
    }]);
    let error = recording
        .mark(ocr_page)
        .expect_err("a select step needs a guard");
    assert_eq!(select_problem(error), SelectProblem::GuardMissing.as_str());
    assert_eq!(recording.recording()["steps"], json!([]), "nothing written");
    // A mark id of the select's generated targets is reserved.
    let mut reserved = base();
    reserved["add"] = json!([{"id": "select/step_01/slot_00", "family": "color",
                              "region": rect(2, 50, 4, 4)}]);
    let error = recording.mark(reserved).expect_err("a reserved id");
    assert_eq!(error.code, "record_mark_id_reserved", "{}", error.message);
}

// The design's "unique in the recording": a second select step with the first one's wanted name
// or its on_empty key is refused; with its own name and key it is accepted. Fix 1: the second
// list is a screen of its own (its page area another gray, its own color mark), since step 1's
// page passing on its frame is refused at mark time (`select_other_page_passes`, a test below).
#[test]
fn r1_a_second_select_has_its_own_wanted_name_and_on_empty_key() {
    let recording = Recording::new();
    let primary = list([Some(ALPHA), Some(BRAVO)]);
    recording
        .mark(recording.step_one(&primary, &[], ("select", select(Some("item_a"), "item_b"))))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    recording.press_open_step();
    let second = |declared: Value| {
        json!({"frame": recording.frame("second", &list_on([Some(BRAVO), Some(ALPHA)], 120)),
               "add": [{"id": "state/second_page", "family": "color", "region": rect(2, 50, 4, 4)}],
               "select": declared})
    };
    // The first one's wanted name with a key of its own, then a name of its own with the first
    // one's key (both the default `neutral_task_not_found`).
    let mut same_name = select(None, "item_a");
    same_name["on_empty_key"] = json!("neutral_task_other_absent");
    let mut same_key = select(None, "item_a");
    same_key["wanted"]["name"] = json!("other_item");
    let cases = [
        (same_name, SelectProblem::WantedNameTaken),
        (same_key, SelectProblem::OnEmptyKeyTaken),
    ];
    for (declared, problem) in cases {
        let before = recording.recording_bytes();
        let error = recording
            .mark(second(declared))
            .expect_err("the name or key is taken");
        assert_eq!(select_problem(error), problem.as_str());
        assert_eq!(recording.recording_bytes(), before, "nothing written");
    }
    let mut own = select(None, "item_a");
    own["wanted"]["name"] = json!("other_item");
    own["on_empty_key"] = json!("neutral_task_other_absent");
    let marked = recording
        .mark(second(own))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    let click = serde_json::to_value(&marked.click).expect("click JSON");
    assert_eq!(click["rect"], json!(SLOTS[1]), "{click}");
}

// The kernel's select_on_empty_target as a recording meets it: on the last step of a stopped
// recording a select is on_final_step; nothing is written.
#[test]
fn r1_a_select_on_the_last_step_of_a_stopped_recording_is_refused() {
    let recording = Recording::new();
    recording
        .mark(recording.step_one(
            &list([Some(ALPHA), Some(BRAVO)]),
            &[],
            ("click", json!({"region": SLOTS[0]})),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    recording.press_open_step();
    recording.mark_next_step(&recording.ocr);
    recording.stop_session();
    let before = recording.recording_bytes();

    let error = recording
        .mark(json!({"step": 2, "select": select(Some("item_a"), "item_b")}))
        .expect_err("the last step");

    assert_eq!(select_problem(error), SelectProblem::OnFinalStep.as_str());
    assert_eq!(recording.recording_bytes(), before, "nothing written");
}

// Item 3 of the brief: a sample frame on which the pressed name stands in another slot is
// accepted; each frame reports its decisions.
#[test]
fn r1_a_sample_with_the_pressed_name_in_another_slot_is_accepted() {
    let recording = Recording::new();
    let sample = list([Some(BRAVO), Some(ALPHA)]);
    let marked = recording
        .mark(recording.step_one(
            &list([Some(ALPHA), Some(BRAVO)]),
            &[&sample],
            ("select", select(Some("item_a"), "item_b")),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    let marked_select = serde_json::to_value(&marked.select).expect("select JSON");
    assert_eq!(
        marked_select["self_test"]["frames"],
        json!([
            decisions("f0001", Some(0), Some(1)),
            decisions("f0002", Some(1), Some(0))
        ]),
        "{marked_select}"
    );
    let click = serde_json::to_value(&marked.click).expect("click JSON");
    assert_eq!(click["rect"], json!(SLOTS[0]), "{click}");
}

// Item 3 of the brief: a recorded press outside the slot the pressed name selects on the primary
// frame is select_press_mismatch; nothing is written.
#[test]
fn r1_a_press_outside_the_selected_slot_is_refused() {
    let recording = Recording::new();
    recording
        .mark(recording.step_one(
            &list([Some(ALPHA), Some(BRAVO)]),
            &[],
            ("click", json!({"region": SLOTS[1]})),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    let point = recording.press_open_step();
    assert!(inside(point, SLOTS[1]), "{point:?}");
    let before = recording.recording_bytes();

    let error = recording
        .mark(json!({"step": 1, "select": select(Some("item_a"), "item_b")}))
        .expect_err("the press hit slot 1");

    let refused = select_rejection(error, "select_press_mismatch");
    assert_eq!(refused["frame_id"], "f0001", "{refused}");
    assert_eq!(refused["slot"], 0, "{refused}");
    assert_eq!(recording.recording_bytes(), before, "nothing written");

    // The press is the demonstration of the name it hit: pressed item_b holds.
    let marked = recording
        .mark(json!({"step": 1, "select": select(Some("item_b"), "item_b")}))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    let click = serde_json::to_value(&marked.click).expect("click JSON");
    assert_eq!(click["source"], "select", "{click}");
    assert_eq!(click["rect"], json!(SLOTS[1]), "{click}");
}

// Item 3 of the brief: a slot whose name reads no known name is select_name_unread with its slot
// and typed cause, and a pressed name the primary frame does not show is select_pressed_absent;
// nothing is written.
#[test]
fn r1_an_unread_slot_or_an_absent_pressed_name_is_refused() {
    let recording = Recording::new();
    let error = recording
        .mark(recording.step_one(
            &list([Some(ALPHA), None]),
            &[],
            ("select", select(Some("item_a"), "item_b")),
        ))
        .expect_err("slot 1 reads nothing");
    let refused = select_rejection(error, "select_name_unread");
    assert_eq!(refused["slot"], 1, "{refused}");
    assert_eq!(refused["cause"], "low_confidence", "{refused}");
    // Fix 1: the default's decision there is no empty end: the unread slot may hold item_b, so
    // the run would fail (blocked), and the refusal says so.
    assert_eq!(
        refused["default"],
        json!({"result": "blocked"}),
        "{refused}"
    );
    assert_eq!(recording.recording()["steps"], json!([]), "nothing written");

    let error = recording
        .mark(recording.step_one(
            &list([Some(BRAVO), Some(BRAVO)]),
            &[],
            ("select", select(Some("item_a"), "item_b")),
        ))
        .expect_err("ALPHA is on no slot");
    let refused = select_rejection(error, "select_pressed_absent");
    assert_eq!(refused["pressed"], "item_a", "{refused}");
    assert_eq!(recording.recording()["steps"], json!([]), "nothing written");
}

// Item 3 of the brief: without an OCR model a select on a step whose press was recorded is
// not_evaluated (lab_ocr_unavailable), never passed, its click kept, and record stop lists it in
// marks_not_evaluated; a select with no press to keep a click from is refused lab_ocr_unavailable.
#[test]
fn r1_without_an_ocr_model_the_select_is_not_evaluated() {
    let fresh = Recording::new();
    let error = fresh
        .mark_with(
            fresh.step_one(
                &list([Some(ALPHA), Some(BRAVO)]),
                &[],
                ("select", select(Some("item_a"), "item_b")),
            ),
            &fresh.none,
        )
        .expect_err("the click comes from the read names");
    assert_eq!(
        error.code,
        LabCode::LabOcrUnavailable.as_str(),
        "{}",
        error.message
    );
    assert_eq!(fresh.recording()["steps"], json!([]), "nothing written");

    let recording = Recording::new();
    recording
        .mark_with(
            recording.step_one(
                &list([Some(ALPHA), Some(BRAVO)]),
                &[],
                ("click", json!({"region": SLOTS[0]})),
            ),
            &recording.none,
        )
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    recording.press_open_step();
    let marked = recording
        .mark_with(
            json!({"step": 1, "select": select(Some("item_a"), "item_b")}),
            &recording.none,
        )
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    let marked_select = serde_json::to_value(&marked.select).expect("select JSON");
    assert_eq!(
        marked_select["self_test"],
        json!({"status": "not_evaluated", "reason": "lab_ocr_unavailable", "frames": []}),
        "{marked_select}"
    );
    let click = serde_json::to_value(&marked.click).expect("click JSON");
    assert_eq!(click["source"], "select", "{click}");
    assert_eq!(click["rect"], json!(SLOTS[0]), "{click}");
    recording.mark_next_step(&recording.none);

    let lab = recording.stop_with(&recording.none);

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    assert_eq!(
        lab["marks_not_evaluated"],
        json!(["select/step_01"]),
        "{lab}"
    );
    assert_eq!(lab["cross_check"]["status"], "partially_evaluated", "{lab}");
    assert_eq!(
        lab["cross_check"]["selects_not_evaluated"],
        json!([1]),
        "{lab}"
    );
    assert_eq!(
        lab["selects"][0]["frames"],
        json!([{"frame_id": "f0001", "result": "undetermined"}]),
        "{lab}"
    );
    assert_eq!(lab["first_decision"]["status"], "not_evaluated", "{lab}");
    assert_eq!(
        lab["first_decision"]["reason"], "lab_ocr_unavailable",
        "{lab}"
    );
}

/// The fake Runtime of one call: it performs the press and its capture takes the next screen.
struct FakeRuntime {
    seen: Vec<String>,
}

impl RecordStepPorts for FakeRuntime {
    fn press(&mut self, plan: &ClickPlan) -> LabResult<CommitClickRequest> {
        self.seen.push(format!("press {}", plan.step));
        Ok(performed())
    }

    fn wait(&mut self, duration: Duration) {
        self.seen.push(format!("wait {}", duration.as_millis()));
    }

    fn capture(&mut self) -> LabResult<AttachFrameRequest> {
        self.seen.push("capture".to_owned());
        Ok(AttachFrameRequest {
            png: next_screen(),
            source: "lab_capture".to_owned(),
            runtime_artifact: None,
            capture_backend: Some("adb_screencap".to_owned()),
            freshness: None,
        })
    }
}

// Item 3 of the brief: L1's one call records a select step: its mark request declares the select,
// the press is planned inside the slot the pressed name selects, performed and recorded, and the
// next frame opens the next step.
#[test]
fn r1_one_call_records_a_select_step() {
    let recording = Recording::new();
    record_attach_frame(
        &recording.lock,
        AttachFrameRequest {
            png: list([Some(BRAVO), Some(ALPHA)]),
            source: "lab_capture".to_owned(),
            runtime_artifact: None,
            capture_backend: Some("adb_screencap".to_owned()),
            freshness: None,
        },
    )
    .unwrap_or_else(|error| panic!("frame: {}", error.code));
    let mark = request(json!({"add": [page_mark()], "select": select(Some("item_a"), "item_b")}));

    let planned = record_plan_step(
        &recording.lock,
        &RecordStepRequest {
            mark: Some(mark),
            click: PlanClickRequest::default(),
            next_frame_after_ms: Some(0),
            next_frame_out: None,
        },
        &recording.ocr,
    )
    .unwrap_or_else(|error| panic!("plan step: {} {:?}", error.code, error.details));
    assert_eq!(planned.plan().rect, SLOTS[1]);
    assert_eq!(planned.plan().source, "select");
    let mut runtime = FakeRuntime { seen: Vec::new() };
    let outcome = planned
        .perform(&recording.lock, &mut runtime)
        .unwrap_or_else(|error| panic!("perform: {} {:?}", error.code, error.details));

    assert_eq!(runtime.seen, ["press 1", "wait 0", "capture"]);
    assert!(outcome.click.step_closed);
    let executed = outcome.click.click.execution.as_ref().expect("the press");
    assert!(
        inside((executed.point.x, executed.point.y), SLOTS[1]),
        "{executed:?}"
    );
    let next = outcome.next_frame.expect("the next frame");
    assert!(next.record.step_opened);
    assert_eq!(next.record.step, 2);
    let steps = recording.recording()["steps"].clone();
    assert_eq!(steps[0]["click"]["source"], "select", "{steps}");
    assert_eq!(
        steps[0]["select"]["self_test"]["frames"],
        json!([decisions("f0001", Some(1), Some(0))]),
        "{steps}"
    );
}

// Item 3 of the brief: the on_empty key is <task_id>_not_found by default (above) and the given
// one when declared; a primary frame without the default name gives the first decision's empty
// end with that key.
#[test]
fn r1_the_on_empty_key_is_the_given_one_and_ends_an_empty_first_decision() {
    let recording = Recording::new();
    let mut declared = select(Some("item_a"), "item_b");
    declared["on_empty_key"] = json!("neutral_task_sold_out");
    let marked = recording
        .mark(recording.step_one(&list([Some(ALPHA), Some(ALPHA)]), &[], ("select", declared)))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    let marked_select = serde_json::to_value(&marked.select).expect("select JSON");
    assert_eq!(
        marked_select["self_test"]["frames"],
        json!([decisions("f0001", Some(0), None)]),
        "{marked_select}"
    );
    recording.press_open_step();
    recording.mark_next_step(&recording.ocr);

    let lab = recording.stop_with(&recording.ocr);

    let entries = recording.package(&lab);
    let task = task(&entries);
    assert_eq!(
        task["scheduling_outcome"]["mappings"][1],
        json!({"outcome_key": "neutral_task_sold_out", "effect": "no_designated_effect",
               "terminal_pages": ["step_01"]}),
        "{task}"
    );
    assert_eq!(
        task["operations"][0]["select"]["on_empty"],
        json!({"outcome_key": "neutral_task_sold_out"}),
        "{task}"
    );
    assert_eq!(
        document(&entries, POLICY)["applies_to"]["outcome_keys"]["empty"],
        "neutral_task_sold_out"
    );
    assert_eq!(
        lab["first_decision"],
        json!({"status": "would_end_empty", "operation_label": "step_01_select",
               "outcome_key": "neutral_task_sold_out", "frame": "f0001"}),
        "{lab}"
    );
}

// Item 3 of the brief: the stopped recording's wanted.default changed by record mark (the select
// declared again), regenerated: the policy's one gate keeps the new default and the package's
// content hash differs.
#[test]
fn r1_a_default_changed_after_stop_regenerates_another_package() {
    let recording = Recording::new();
    recording
        .mark(recording.step_one(
            &list([Some(ALPHA), Some(BRAVO)]),
            &[],
            ("select", select(Some("item_a"), "item_b")),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    recording.press_open_step();
    recording.mark_next_step(&recording.ocr);
    let generated = recording.stop_session();
    let before = hex_sha256(&fs::read(generated["path"].as_str().expect("path")).expect("package"));

    let marked = recording
        .mark(json!({"step": 1, "select": select(Some("item_a"), "item_a")}))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    assert!(marked.after_stop);
    let regenerated = recording.stop_with(&recording.ocr);

    assert_eq!(regenerated["lab_status"], "regenerated", "{regenerated}");
    let after =
        hex_sha256(&fs::read(regenerated["path"].as_str().expect("path")).expect("package"));
    assert_ne!(after, before, "the package's content changed");
    let entries = recording.package(&regenerated);
    assert_eq!(
        document(&entries, POLICY)["gates"][0]["predicate"]["allowed"],
        json!(["item_a"])
    );
}

// Item 3 of the brief: a recording without a select has no select anywhere: no step key, no layout,
// no policy document, one mapping, a click operation and no `selects`. Fix 1 (item 9): no existing
// test compares a generated package with a constant (gate_e5_record_stop compares two runs of one
// build), so this test holds one: the package's files, its control and resource documents, and
// its task without the task's provenance (which carries the recording's write times), as
// lab-recording.md "Package generation" states them for this recording.
#[test]
fn r1_a_recording_without_a_select_generates_as_before() {
    let recording = Recording::new();
    recording
        .mark(recording.step_one(
            &list([Some(ALPHA), Some(BRAVO)]),
            &[],
            ("click", json!({"region": SLOTS[0]})),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    recording.press_open_step();
    recording.mark_next_step(&recording.ocr);

    let lab = recording.stop_with(&recording.ocr);

    let text = String::from_utf8(recording.recording_bytes()).expect("recording text");
    assert!(!text.contains("\"select\""), "{text}");
    let entries = recording.package(&lab);
    assert!(
        entries.keys().all(|path| !path.contains("policies/")),
        "{:?}",
        entries.keys()
    );
    let task = task(&entries);
    assert_eq!(task.get("candidate_layouts"), None, "{task}");
    assert_eq!(task.get("ocr_targets"), None, "{task}");
    assert_eq!(task["operations"][0]["id"], "step_01_click", "{task}");
    assert_eq!(
        task["scheduling_outcome"]["mappings"],
        json!([{"outcome_key": "neutral_task_done", "effect": "no_designated_effect",
                "terminal_pages": ["step_02"]}]),
        "{task}"
    );
    assert_eq!(lab.get("selects"), None, "{lab}");
    assert_eq!(lab["cross_check"].get("selects"), None, "{lab}");

    assert_eq!(
        entries.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "control.json",
            "resources/operations/neutral_task/assets/ui_next.png",
            "resources/operations/neutral_task/task.json",
            "resources/operations/resources.json",
        ]
    );
    assert_eq!(
        document(&entries, "control.json"),
        json!({
            "schema_version": "Lab-1y.control.v2",
            "package_id": "neutral.test.neutral_task",
            "execution_mode": "linear_steps",
            "game": "neutral",
            "server": "test",
            "resolution": {"width": WIDTH, "height": HEIGHT},
            "entry_task_id": TASK_ID,
            "timeout_ms": 40_200,
            "step_timeout_ms": 15_000,
            "max_steps": 1
        })
    );
    assert_eq!(
        document(&entries, "resources/operations/resources.json"),
        json!({"schema_version": "1.0", "resources": [], "resource_count": 0})
    );
    let frame_sha256 = recording.recording()["steps"][0]["frames"][0]["sha256"].clone();
    let mut without_provenance = task;
    without_provenance
        .as_object_mut()
        .expect("task object")
        .remove("provenance")
        .expect("the task's provenance");
    let mark_provenance = |step: u32| {
        json!({"source": "lab_record", "record_id": RECORD_ID, "step": step,
               "record_index": step, "self_test": "passed"})
    };
    assert_eq!(
        without_provenance,
        json!({
            "schema_version": "0.9",
            "task_id": TASK_ID,
            "game": "neutral",
            "server_scope": ["test"],
            "locale": "en-US",
            "goal": "Lab recording record_r1: 1 click step(s)",
            "coordinate_space": {"width": WIDTH, "height": HEIGHT},
            "defaults": {"template_threshold": 0.95, "color_max_distance": 20,
                         "match_metric": "ccoeff_normed"},
            "timeout_ms": 40_200,
            "max_steps": 1,
            "entry_page": "step_01",
            "target_page": "step_02",
            "scheduling_outcome": {"mappings": [{"outcome_key": "neutral_task_done",
                                                 "effect": "no_designated_effect",
                                                 "terminal_pages": ["step_02"]}]},
            "verify_templates": [{"id": "ui/next", "template": "assets/ui_next.png",
                                  "region": {"mode": "rect", "rect": rect(40, 46, 12, 12)},
                                  "provenance": mark_provenance(2)}],
            "color_probes": [{"id": PAGE,
                              "region": {"mode": "rect", "rect": rect(2, 50, 4, 4)},
                              "expected": [BACKGROUND, BACKGROUND, BACKGROUND],
                              "provenance": mark_provenance(1)}],
            "page_rules": {"step_01": {"required": [PAGE]}, "step_02": {"required": ["ui/next"]}},
            "operations": [{
                "id": "step_01_click",
                "purpose": "Lab recording step 1",
                "from": "step_01",
                "to": "step_02",
                "click": {"kind": "rect", "x": 8, "y": 8, "width": 32, "height": 32},
                "guard": {"page_id": "step_01", "target_id": PAGE,
                          "expected_rect": rect(2, 50, 4, 4), "color_probe": PAGE},
                "expect_after": {"page_id": "step_02", "timeout_ms": 15_000, "interval_ms": 500},
                "post_delay_ms": 200,
                "provenance": {"source": "lab_record", "record_id": RECORD_ID, "step": 1,
                               "record_index": 1, "frame_sha256": frame_sha256}
            }]
        })
    );
}

// The files of a package, the task's provenance without its generation time (the time the
// recording was last written, which every `record mark` changes).
fn without_generation_time(mut entries: BTreeMap<String, Vec<u8>>) -> BTreeMap<String, Vec<u8>> {
    for (name, bytes) in &mut entries {
        if !name.ends_with("/task.json") {
            continue;
        }
        let mut task: Value = serde_json::from_slice(bytes).expect("task JSON");
        task["provenance"]
            .as_object_mut()
            .expect("provenance")
            .remove("generated_at_unix_ms")
            .expect("the generation time");
        *bytes = serde_json::to_vec(&task).expect("task bytes");
    }
    entries
}

// Fix 1, item 1: a select on the application entry step, which has no frame, is never dropped:
// with `optional: false` beside it, or alone, it is record_select_invalid with with_application,
// as a select beside an application operation is, and nothing is written.
#[test]
fn r1_fix1_a_select_on_an_application_step_is_refused() {
    let recording = Recording::new();
    recording
        .mark(json!({"application": {"action": "launch"}}))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    let before = recording.recording_bytes();
    for value in [
        json!({"step": 1, "select": select(Some("item_a"), "item_b"), "optional": false}),
        json!({"step": 1, "select": select(Some("item_a"), "item_b")}),
    ] {
        let error = recording
            .mark(value)
            .expect_err("an application step takes no select");
        assert_eq!(
            select_problem(error),
            SelectProblem::WithApplication.as_str()
        );
        assert_eq!(recording.recording_bytes(), before, "nothing written");
    }
}

// Fix 1, item 3: after record stop a select turns the recorded tap of step 1 into a choice by name
// (the tap lies in slot 0, where the pressed name item_a stands), the tap's own rectangle and
// source kept beside it in recording.json; clear_select gives them back, the step is the one
// recorded before, and the next record stop regenerates the package it generated before.
#[test]
fn r1_fix1_a_select_added_after_stop_and_taken_off_generates_the_package_again() {
    let recording = Recording::new();
    recording
        .mark(recording.step_one(
            &list([Some(ALPHA), Some(BRAVO)]),
            &[],
            ("click", json!({"region": SLOTS[0]})),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    recording.press_open_step();
    recording.mark_next_step(&recording.ocr);
    let generated = recording.stop_session();
    let steps = recording.recording()["steps"].clone();
    let package = without_generation_time(recording.package(&generated));

    let selected = recording
        .mark(json!({"step": 1, "select": select(Some("item_a"), "item_b")}))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    assert!(selected.after_stop);
    let click = serde_json::to_value(&selected.click).expect("click JSON");
    assert_eq!(click["source"], "select", "{click}");
    assert_eq!(click["rect"], json!(SLOTS[0]), "{click}");
    assert_eq!(
        recording.recording()["steps"][0]["select"]["click_before"],
        json!({"rect": SLOTS[0], "source": "declared", "from": null})
    );

    let cleared = recording
        .mark(json!({"step": 1, "clear_select": true}))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    assert!(cleared.after_stop);
    assert!(cleared.select.is_none(), "{:?}", cleared.select);
    assert_eq!(
        recording.recording()["steps"],
        steps,
        "the step recorded before"
    );
    let regenerated = recording.stop_with(&recording.ocr);

    assert_eq!(regenerated["lab_status"], "regenerated", "{regenerated}");
    assert_eq!(
        without_generation_time(recording.package(&regenerated)),
        package
    );
}

// Fix 1, item 7: a select on a list whose frame step 1's page passes on too (the second step
// reuses step 1's only mark) is refused at mark time with select_other_page_passes, naming step 1
// and the frame, as record stop would find it (self-check item 8); nothing is written.
#[test]
fn r1_fix1_a_select_on_a_frame_another_page_passes_on_is_refused() {
    let recording = Recording::new();
    recording
        .mark(recording.step_one(
            &list([Some(ALPHA), Some(BRAVO)]),
            &[],
            ("click", json!({"region": SLOTS[0]})),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    recording.press_open_step();
    let before = recording.recording_bytes();

    let error = recording
        .mark(
            json!({"frame": recording.frame("second", &list([Some(BRAVO), Some(ALPHA)])),
                     "reuse": [PAGE], "select": select(Some("item_a"), "item_b")}),
        )
        .expect_err("step 1's page passes on the list");

    let refused = select_rejection(error, "select_other_page_passes");
    assert_eq!(refused["step"], 1, "{refused}");
    assert_eq!(refused["frame_id"], "f0002", "{refused}");
    assert_eq!(refused["transition"], false, "{refused}");
    assert_eq!(recording.recording_bytes(), before, "nothing written");
}

// Fix 1, item 6: the select step the mark-time probe admits is the one record stop generates. For
// one recording, the probe built from the stopped recording's step holds the package's policy
// document byte for byte, its layout and slot targets, and its operation but for expect_after's
// timeout, which the probe takes at the largest a package declares.
#[test]
fn r1_fix1_the_probe_holds_the_select_step_record_stop_generates() {
    let recording = Recording::new();
    recording
        .mark(recording.step_one(
            &list([Some(ALPHA), Some(BRAVO)]),
            &[],
            ("select", select(Some("item_a"), "item_b")),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    recording.press_open_step();
    recording.mark_next_step(&recording.ocr);
    let lab = recording.stop_with(&recording.ocr);
    let entries = recording.package(&lab);
    let stopped: LabRecording =
        serde_json::from_slice(&recording.recording_bytes()).expect("the recording");
    let step = stopped.steps[0].clone();

    let plan = select_probe_plan(&stopped, &step, step.marks.clone())
        .unwrap_or_else(|error| panic!("probe plan: {} {:?}", error.code, error.details));
    let probe = render(&plan, None, &BTreeMap::new())
        .unwrap_or_else(|error| panic!("probe render: {} {:?}", error.code, error.details));

    assert_eq!(
        probe.get(POLICY).expect("the probe's policy document"),
        entries.get(POLICY).expect("the package's policy document")
    );
    let (probe_task, package_task) = (task(&probe), task(&entries));
    for member in ["candidate_layouts", "ocr_targets"] {
        assert_eq!(probe_task[member], package_task[member], "{member}");
    }
    let operation = |task: &Value| {
        let mut operation = task["operations"][0].clone();
        operation["expect_after"]
            .as_object_mut()
            .expect("expect_after")
            .remove("timeout_ms")
            .expect("a timeout");
        operation
    };
    assert_eq!(package_task["operations"][0]["id"], "step_01_select");
    assert_eq!(operation(&probe_task), operation(&package_task));
}
