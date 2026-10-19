// SPDX-License-Identifier: AGPL-3.0-only

// #411 Lab B-9 (exec key 108; register §1.132): a Lab recording marks a value test. A
// `value_test` mark (request form only) declares a left side, a comparison and a right side, each
// side an inline OCR read; its self-test evaluates the Runtime's value test with the Lab's OCR
// provider on every live frame of the step, and without an OCR model it is not evaluated, never
// passed. `recording.json` keeps the declaration; a check may name the mark and `--click-guard`
// may name it. `record stop` declares one OCR target per side (`<id>/left`, `<id>/right`) and the
// value test as a task 0.9 `checks[]` entry, which the page requires and a guard may name; the
// first decision and the cross-check count it as OCR. The mark is refused where the Runtime
// refuses a value test: in `forbidden`, as an anchor, as a click source and as a followed
// template.
//
// Frames are synthetic: 96x64 pixels of flat gray with neutral noise patches. The scripted OCR
// provider reads a text chosen by the gray value of the top-left pixel of the region it reads.
// Requirement basis: plan-lab-l5 §2 B-9, R1 to R6, with coordinator decision Q10.

use crate::recording::crosscheck::admit;
use crate::recording::store::{old_record_path, recording_path};
use crate::{
    ClickSpec, LAB_RECORD_MARK_SCHEMA, LabOcr, MarkFamily, MarkOutcome, MarkRequest, MarkSpec,
    RecordRect, RecordStartOptions, RecordStopOptions, RecordingLock, SelfTestStatus, record_mark,
    record_start, record_status, record_stop,
};
use actingcommand_contract::{InputAction, LabError};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_execution_kernel::{OfflineDecision, simulate_contained_task};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, VisionProvider, VisionProviderError, VisionProviderErrorCode,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_b9";
const TASK_ID: &str = "neutral_task";
const WIDTH: u32 = 96;
const HEIGHT: u32 = 64;
const BACKGROUND: u8 = 60;
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

// The template of step 1 and the template of step 2, each searched at its crop.
const BUTTON: RecordRect = RecordRect {
    x: 8,
    y: 8,
    width: 12,
    height: 12,
};
const NEXT: RecordRect = RecordRect {
    x: 8,
    y: 40,
    width: 12,
    height: 12,
};
// The tap of step 1.
const TAP: RecordRect = RecordRect {
    x: 26,
    y: 24,
    width: 8,
    height: 8,
};
// The regions the two sides of a value test read.
const LEFT: RecordRect = RecordRect {
    x: 50,
    y: 4,
    width: 40,
    height: 10,
};
const RIGHT: RecordRect = RecordRect {
    x: 44,
    y: 30,
    width: 40,
    height: 10,
};

// The gray value of a region's top-left pixel that makes the scripted provider read each text.
const READS_25000: u8 = 101;
const READS_25310: u8 = 102;
const READS_24999: u8 = 103;
const READS_2A5: u8 = 104;
const READS_3_OF_5: u8 = 105;
const READS: [(u8, &str); 5] = [
    (READS_25000, "25000"),
    (READS_25310, "25310"),
    (READS_24999, "24999"),
    (READS_2A5, "2a5"),
    (READS_3_OF_5, "3 / 5"),
];

// Reads the text the top-left pixel of the requested region selects (`READS`), at confidence
// 0.95; any other pixel reads nothing.
#[derive(Debug)]
struct ScriptedOcr;

impl VisionProvider for ScriptedOcr {
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
        let index = (request.region.y as usize * request.frame.width as usize
            + request.region.x as usize)
            * 3;
        let pixel = request.frame.rgb8_pixels.get(index).copied();
        let text = READS
            .iter()
            .find(|(value, _)| Some(*value) == pixel)
            .map_or("", |(_, text)| *text);
        let blocks = if text.is_empty() {
            Vec::new()
        } else {
            vec![OcrProviderTextBlock {
                text: text.to_owned(),
                rect: request.region,
                confidence: Some(0.95),
            }]
        };
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: text.to_owned(),
            blocks,
            confidence: Some(0.95),
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

// A deterministic generator of neutral gray noise.
struct Noise(u64);

impl Noise {
    // A value in `low..=high`.
    fn between(&mut self, low: u8, high: u8) -> u8 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        low + ((self.0 >> 33) % (u64::from(high - low) + 1)) as u8
    }
}

// A gray image, row-major; it is drawn with equal red, green and blue.
#[derive(Clone)]
struct Gray {
    width: u32,
    height: u32,
    values: Vec<u8>,
}

impl Gray {
    fn from_fn(width: u32, height: u32, mut value: impl FnMut(u32, u32) -> u8) -> Self {
        let mut values = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            for x in 0..width {
                values.push(value(x, y));
            }
        }
        Self {
            width,
            height,
            values,
        }
    }

    fn frame() -> Self {
        Self::from_fn(WIDTH, HEIGHT, |_, _| BACKGROUND)
    }

    fn noise(seed: u64) -> Self {
        let mut noise = Noise(seed);
        Self::from_fn(12, 12, |_, _| noise.between(1, 255))
    }

    // `image` drawn with its top-left corner at the rectangle's.
    fn draw(mut self, at: RecordRect, image: &Gray) -> Self {
        for row in 0..image.height {
            for column in 0..image.width {
                self.values[((at.y as u32 + row) * self.width + at.x as u32 + column) as usize] =
                    image.values[(row * image.width + column) as usize];
            }
        }
        self
    }

    // The top-left pixel of `region` set to `value`, which selects what the provider reads there.
    fn reads(mut self, region: RecordRect, value: u8) -> Self {
        self.values[(region.y as u32 * self.width + region.x as u32) as usize] = value;
        self
    }

    fn png(&self) -> Vec<u8> {
        let pixels = self
            .values
            .iter()
            .flat_map(|value| [*value; 3])
            .collect::<Vec<_>>();
        Frame::from_pixels(
            self.width,
            self.height,
            pixels,
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("fixture frame")
        .encode_png_fast()
        .expect("fixture PNG")
    }
}

fn rect(x: i32, y: i32, width: i32, height: i32) -> RecordRect {
    RecordRect {
        x,
        y,
        width,
        height,
    }
}

// Step 1's screen: the button; step 2's screen: the next mark.
fn first() -> Gray {
    Gray::frame().draw(BUTTON, &Gray::noise(0x0411_0b91))
}

fn second() -> Gray {
    Gray::frame().draw(NEXT, &Gray::noise(0x0411_0b92))
}

fn template(id: &str, crop: RecordRect) -> MarkSpec {
    MarkSpec {
        search: Some(crop),
        ..MarkSpec::region_only(id.to_owned(), MarkFamily::Template, crop)
    }
}

fn all_of(id: &str, members: &[&str]) -> MarkSpec {
    MarkSpec {
        region: None,
        all_of: Some(members.iter().map(|member| (*member).to_owned()).collect()),
        ..MarkSpec::region_only(id.to_owned(), MarkFamily::Check, BUTTON)
    }
}

fn tap() -> ClickSpec {
    ClickSpec {
        region: Some(TAP),
        ..ClickSpec::default()
    }
}

// A decimal value in 0..=99999.
fn decimal() -> Value {
    json!({"type": "unsigned_integer", "min": 0, "max": 99999})
}

// A `current_capacity` value in 0..=99.
fn capacity() -> Value {
    json!({"type": "unsigned_integer", "min": 0, "max": 99, "format": "current_capacity"})
}

// One side of a value test, as the request declares it: an OCR read of `region`.
fn side(region: RecordRect, value: Value) -> Value {
    json!({
        "region": region,
        "languages": ["en"],
        "timeout_ms": 1000,
        "minimum_confidence": 0.8,
        "model_ref": "neutral-ocr",
        "model_sha256": MODEL_SHA256,
        "trim": "whitespace_v1",
        "value": value
    })
}

// A value-test mark in the request form.
fn value_test(id: &str, left: Value, op: &str, right: Option<Value>) -> MarkSpec {
    let mut spec = json!({"id": id, "family": "value_test", "left": left, "op": op});
    if let Some(right) = right {
        spec["right"] = right;
    }
    serde_json::from_value(spec).expect("a value-test mark request")
}

// `value/count >= 25000`, its left side reading LEFT.
fn count_at_least_25000() -> MarkSpec {
    value_test(
        "value/count",
        side(LEFT, decimal()),
        "ge",
        Some(json!({"const": 25000})),
    )
}

// A state directory with an active session of `INSTANCE` and its Lab recording, before any
// step; with the scripted OCR provider, or without an OCR model. The lock is dropped before the
// directory.
struct Recording {
    lock: RecordingLock,
    ocr: LabOcr,
    temp: TempDir,
}

impl Recording {
    fn new(with_ocr: bool) -> Self {
        let temp = TempDir::new().expect("temporary directory");
        let state = temp.path().join("state");
        fs::create_dir_all(&state).expect("state directory");
        // The session file `record start` writes before the Lab recording is created.
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
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_lab_b9").expect("recording lock");
        let ocr = if with_ocr {
            LabOcr::with_provider(Arc::new(ScriptedOcr))
        } else {
            LabOcr::from_vision_root(&temp.path().join("no-vision"))
        };
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
        Self { lock, ocr, temp }
    }

    fn state(&self) -> PathBuf {
        self.temp.path().join("state")
    }

    // Writes `image` as the frame file `name` and returns its path.
    fn frame(&self, name: &str, image: &Gray) -> String {
        let path = self.temp.path().join(format!("{name}.png"));
        fs::write(&path, image.png()).expect("frame file");
        path.display().to_string()
    }

    fn mark(&self, request: MarkRequest) -> Result<MarkOutcome, LabError> {
        record_mark(&self.lock, &request, false, &self.ocr)
    }

    // A step with its own frame, marks and click.
    fn step(
        &self,
        name: &str,
        image: &Gray,
        add: Vec<MarkSpec>,
        click: Option<ClickSpec>,
    ) -> MarkRequest {
        MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            frame: Some(self.frame(name, image)),
            add,
            click,
            ..MarkRequest::default()
        }
    }

    // A request on the recorded step `step`.
    fn on_step(&self, step: u32) -> MarkRequest {
        MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            step: Some(step),
            ..MarkRequest::default()
        }
    }

    // Step 2: the final screen with its own mark.
    fn mark_second(&self) -> MarkOutcome {
        self.mark(self.step("second", &second(), vec![template("ui/next", NEXT)], None))
            .unwrap_or_else(|error| panic!("step 2: {} {:?}", error.code, error.details))
    }

    // `record stop`: the `lab` output with its status.
    fn stop(&self) -> Value {
        let outcome = record_stop(&self.lock, &RecordStopOptions::default(), &self.ocr)
            .unwrap_or_else(|error| {
                panic!(
                    "record stop: {}: {} {:?}",
                    error.code, error.message, error.details
                )
            });
        let mut lab = serde_json::to_value(&outcome.lab).expect("lab JSON");
        lab["lab_status"] = json!(outcome.lab_status);
        lab
    }

    // The files of the package `record stop` wrote (a template makes it a ZIP).
    fn package(&self, lab: &Value) -> BTreeMap<String, Vec<u8>> {
        let path = lab["path"].as_str().expect("package path");
        let bytes = fs::read(path).expect("package file");
        expand_content_container(&bytes, ContentContainer::Zip, ContainmentLimits::default())
            .expect("package container")
    }

    fn recording(&self) -> Value {
        serde_json::from_slice(&self.recording_bytes()).expect("recording")
    }

    fn recording_bytes(&self) -> Vec<u8> {
        fs::read(recording_path(&self.state(), RECORD_ID)).expect("recording file")
    }
}

fn task(entries: &BTreeMap<String, Vec<u8>>) -> Value {
    let bytes = entries
        .get(&format!("resources/operations/{TASK_ID}/task.json"))
        .expect("task.json");
    serde_json::from_slice(bytes).expect("task JSON")
}

// The input the admitted package decides on `image` for its first step.
fn simulated(
    recording: &Recording,
    entries: &BTreeMap<String, Vec<u8>>,
    image: &Gray,
) -> InputAction {
    let prepared =
        admit(entries, &recording.ocr).unwrap_or_else(|error| panic!("admission: {}", error.code));
    let frame = Frame::from_png(image.png(), CaptureBackendName::AdbScreencap).expect("frame");
    let result = simulate_contained_task(&prepared, vec![frame]).expect("simulation");
    match result.decision {
        OfflineDecision::WouldClick {
            operation_label,
            action,
            ..
        } => {
            assert_eq!(operation_label, "step_01_click");
            action
        }
        other => panic!("not a click: {other:?}"),
    }
}

fn tapped_inside(action: InputAction, rect: RecordRect) -> bool {
    let InputAction::Tap { x, y } = action else {
        panic!("the first decision is not a tap: {action:?}");
    };
    x >= rect.x && y >= rect.y && x < rect.x + rect.width && y < rect.y + rect.height
}

// The refused marks of a `record_mark_rejected`.
fn rejected_marks(error: LabError) -> Value {
    assert_eq!(error.code, "record_mark_rejected", "{}", error.message);
    error.details.expect("details")["marks"].clone()
}

// The entry of `id` in a JSON array of marks.
fn entry(marks: &Value, id: &str) -> Value {
    marks
        .as_array()
        .expect("marks")
        .iter()
        .find(|mark| mark["id"] == id)
        .cloned()
        .unwrap_or_else(|| panic!("no mark {id} in {marks}"))
}

// R1-R3: a value test reads "25000" on the primary frame and "25310" on a sample, and `>= 25000`
// holds on both: its self-test passes on both frames, and a check that names it with a template
// passes with both members. `recording.json` keeps the declaration under `value_test`, without a
// region, and `record status` shows the mark with it.
#[test]
fn b9_a_value_test_mark_that_holds_on_every_frame_is_recorded() {
    let recording = Recording::new(true);

    let marked = recording
        .mark(MarkRequest {
            samples: vec![recording.frame("sample", &first().reads(LEFT, READS_25310))],
            ..recording.step(
                "first",
                &first().reads(LEFT, READS_25000),
                vec![
                    template("ui/button", BUTTON),
                    count_at_least_25000(),
                    all_of("check/ready", &["ui/button", "value/count"]),
                ],
                Some(tap()),
            )
        })
        .unwrap_or_else(|error| panic!("step 1: {} {:?}", error.code, error.details));

    let declared = json!({"left": side(LEFT, decimal()), "op": "ge", "right": {"const": 25000}});
    let marks = serde_json::to_value(&marked.marks).expect("marks JSON");
    let count = entry(&marks, "value/count");
    assert_eq!(count["family"], "value_test");
    assert_eq!(count["value_test"], declared);
    assert_eq!(
        count["self_test"],
        json!({"status": "passed", "frames": 2, "single_sample": false})
    );
    let check = entry(&marks, "check/ready");
    assert_eq!(check["self_test"]["status"], "passed");
    assert_eq!(
        check["self_test"]["members"],
        json!({"ui/button": "passed", "value/count": "passed"})
    );

    let stored = entry(&recording.recording()["steps"][0]["marks"], "value/count");
    assert_eq!(stored["family"], "value_test");
    assert_eq!(stored["value_test"], declared);
    assert!(stored.get("region").is_none(), "{stored}");

    let status =
        serde_json::to_value(record_status(&recording.state(), INSTANCE).expect("record status"))
            .expect("status JSON");
    let shown = entry(&status["steps"][0]["marks"], "value/count");
    assert_eq!(shown["family"], "value_test");
    assert_eq!(shown["self_test"]["status"], "passed");
    assert_eq!(shown["value_test"], declared);
}

// R2: a frame on which the value test does not hold refuses the batch with self_test_failed and
// the evaluation's failure: "24999" is value_test_not_met for `ge`, "2a5" is value_test_unresolved
// on the left side with value_problem invalid. Nothing is written.
#[test]
fn b9_a_value_test_that_does_not_hold_or_read_refuses_the_batch() {
    let recording = Recording::new(true);
    let before = recording.recording_bytes();

    for (read, code, values) in [
        (READS_24999, "value_test_not_met", json!({"value_op": "ge"})),
        (
            READS_2A5,
            "value_test_unresolved",
            json!({"value_side": "left", "value_problem": "invalid"}),
        ),
    ] {
        let error = recording
            .mark(recording.step(
                "first",
                &first().reads(LEFT, read),
                vec![template("ui/button", BUTTON), count_at_least_25000()],
                Some(tap()),
            ))
            .expect_err("the value test does not hold");

        let rejection = entry(&rejected_marks(error), "value/count");
        assert_eq!(rejection["reason"], "self_test_failed", "{rejection}");
        assert_eq!(rejection["frame_id"], "f0001", "{rejection}");
        assert_eq!(rejection["failure"]["code"], code, "{rejection}");
        for (key, value) in values.as_object().expect("values") {
            assert_eq!(&rejection["failure"]["values"][key], value, "{rejection}");
        }
        assert_eq!(recording.recording_bytes(), before, "nothing is written");
    }
}

// R2, R6: without an OCR model the value test is not evaluated (lab_ocr_unavailable), never
// passed; record stop does not simulate a step 1 that requires it (first decision not_evaluated)
// and the cross-check leaves step 1's page undetermined on its frame.
#[test]
fn b9_without_an_ocr_model_a_value_test_is_not_evaluated() {
    let recording = Recording::new(false);
    let marked = recording
        .mark(recording.step(
            "first",
            &first().reads(LEFT, READS_25000),
            vec![template("ui/button", BUTTON), count_at_least_25000()],
            Some(tap()),
        ))
        .unwrap_or_else(|error| panic!("step 1: {} {:?}", error.code, error.details));
    let count = marked
        .marks
        .iter()
        .find(|mark| mark.id == "value/count")
        .expect("the value test");
    assert_eq!(count.self_test.status, SelfTestStatus::NotEvaluated);
    assert_eq!(
        count.self_test.reason.as_deref(),
        Some("lab_ocr_unavailable")
    );
    recording.mark_second();

    let lab = recording.stop();

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    assert_eq!(
        lab["first_decision"],
        json!({"status": "not_evaluated", "reason": "lab_ocr_unavailable"})
    );
    assert_eq!(lab["cross_check"]["status"], "partially_evaluated");
    assert_eq!(
        lab["cross_check"]["self"][0],
        json!({
            "step": 1,
            "page": "step_01",
            "transition": false,
            "frames": [{"frame_id": "f0001", "result": "undetermined"}]
        })
    );
    assert_eq!(lab["marks_not_evaluated"], json!(["value/count"]));
}

// R5, R6: record stop declares one OCR target per side, `<id>/left` and `<id>/right`, with the
// side's region, languages, timeout, minimum confidence and model, `contains` any ASCII digit,
// case-insensitive; and the value test as a `checks[]` entry whose sides name those targets with
// their trim, value and part, the right side with mul and add. The page requires the value test,
// never its sides. The kernel admits the package, and with OCR the first decision taps and the
// cross-check passes on step 1's own frame ("3 / 5": 3 <= 5 * 2 - 5).
#[test]
fn b9_record_stop_generates_the_value_test_and_its_side_targets() {
    let recording = Recording::new(true);
    let left = {
        let mut left = side(LEFT, capacity());
        left["trim"] = json!("whitespace_all_v1");
        left["part"] = json!("current");
        left
    };
    let right = {
        let mut right = side(RIGHT, capacity());
        right["trim"] = json!("whitespace_all_v1");
        right["part"] = json!("capacity");
        right["mul"] = json!(2);
        right["add"] = json!(-5);
        right
    };
    let image = first().reads(LEFT, READS_3_OF_5).reads(RIGHT, READS_3_OF_5);
    let marked = recording
        .mark(recording.step(
            "first",
            &image,
            vec![
                template("ui/button", BUTTON),
                value_test("value/pair", left, "le", Some(right)),
            ],
            Some(tap()),
        ))
        .unwrap_or_else(|error| panic!("step 1: {} {:?}", error.code, error.details));
    assert_eq!(marked.marks[1].self_test.status, SelfTestStatus::Passed);
    recording.mark_second();

    let lab = recording.stop();

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let entries = recording.package(&lab);
    let task = task(&entries);
    let side_target = |id: &str, region: RecordRect| {
        json!({
            "id": id,
            "region": {"mode": "rect", "rect": region},
            "languages": ["en"],
            "timeout_ms": 1000,
            "match_mode": "contains",
            "expected": ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"],
            "case_sensitive": false,
            "minimum_confidence": 0.8,
            "model_ref": "neutral-ocr",
            "model_sha256": MODEL_SHA256
        })
    };
    assert_eq!(
        task["ocr_targets"],
        json!([
            side_target("value/pair/left", LEFT),
            side_target("value/pair/right", RIGHT)
        ])
    );
    assert_eq!(
        task["checks"],
        json!([{
            "id": "value/pair",
            "value_test": {
                "left": {
                    "target_id": "value/pair/left",
                    "trim": "whitespace_all_v1",
                    "value": capacity(),
                    "part": "current"
                },
                "op": "le",
                "right": {
                    "target_id": "value/pair/right",
                    "trim": "whitespace_all_v1",
                    "value": capacity(),
                    "part": "capacity",
                    "mul": 2,
                    "add": -5
                }
            }
        }])
    );
    assert_eq!(
        task["page_rules"],
        json!({
            "step_01": {"required": ["ui/button", "value/pair"]},
            "step_02": {"required": ["ui/next"]}
        })
    );
    admit(&entries, &recording.ocr).unwrap_or_else(|error| panic!("admission: {}", error.code));
    assert_eq!(lab["cross_check"]["status"], "passed", "{lab}");
    assert_eq!(
        lab["cross_check"]["self"][0]["frames"],
        json!([{"frame_id": "f0001", "result": "match"}])
    );
    assert!(tapped_inside(simulated(&recording, &entries, &image), TAP));
}

// R3, R5: `--click-guard` names the value test: the click's guard is the value test as a check
// (`check` its id) whose `expected_rect` is the bounding box of both sides' regions, and the
// admitted package taps on step 1's frame, where the value test holds.
#[test]
fn b9_a_value_test_guards_the_click() {
    let recording = Recording::new(true);
    let mut right = side(RIGHT, decimal());
    right["mul"] = json!(1);
    right["add"] = json!(0);
    let image = first().reads(LEFT, READS_25000).reads(RIGHT, READS_25000);
    recording
        .mark(MarkRequest {
            click_guard: Some("value/count".to_owned()),
            ..recording.step(
                "first",
                &image,
                vec![
                    template("ui/button", BUTTON),
                    value_test("value/count", side(LEFT, decimal()), "ge", Some(right)),
                ],
                Some(tap()),
            )
        })
        .unwrap_or_else(|error| panic!("step 1: {} {:?}", error.code, error.details));
    recording.mark_second();

    let lab = recording.stop();

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let entries = recording.package(&lab);
    assert_eq!(
        task(&entries)["operations"][0]["guard"],
        json!({
            "page_id": "step_01",
            "target_id": "value/count",
            "expected_rect": rect(44, 4, 46, 36),
            "check": "value/count"
        })
    );
    assert!(tapped_inside(simulated(&recording, &entries, &image), TAP));
}

// R4, R1: the value test is refused where the Runtime refuses one: another page cannot forbid it
// (validation_failed with its id and family), it is no anchor (anchor_invalid, not_own_template),
// no click source (record_click_source_invalid: it has no region) and no followed template
// (record_click_follow_invalid, not_template). A side target id that another mark id takes is
// record_mark_id_conflict in both directions, and `mul` on the left side is validation_failed.
#[test]
fn b9_value_tests_are_refused_where_the_runtime_refuses_them() {
    let recording = Recording::new(false);
    recording
        .mark(recording.step(
            "first",
            &first().reads(LEFT, READS_25000),
            vec![template("ui/button", BUTTON), count_at_least_25000()],
            Some(tap()),
        ))
        .unwrap_or_else(|error| panic!("step 1: {} {:?}", error.code, error.details));
    let before = recording.recording_bytes();
    let step_two = |add: Vec<MarkSpec>| recording.step("second", &second(), add, None);

    let forbid = recording
        .mark(MarkRequest {
            forbid: vec!["value/count".to_owned()],
            ..step_two(vec![template("ui/next", NEXT)])
        })
        .expect_err("a page cannot forbid a value test");
    assert_eq!(forbid.code, "validation_failed", "{}", forbid.message);
    assert_eq!(
        forbid.details,
        Some(json!({"mark_id": "value/count", "family": "value_test"}))
    );

    let anchor = recording
        .mark(MarkRequest {
            anchor: Some("value/count".to_owned()),
            ..recording.on_step(1)
        })
        .expect_err("a value test is no anchor");
    assert_eq!(
        rejected_marks(anchor),
        json!([{"id": "value/count", "reason": "anchor_invalid", "problem": "not_own_template"}])
    );

    let source = recording
        .mark(MarkRequest {
            click: Some(ClickSpec {
                from: Some("value/count".to_owned()),
                ..ClickSpec::default()
            }),
            replace_click: true,
            ..recording.on_step(1)
        })
        .expect_err("a value test has no region to click");
    assert_eq!(
        source.code, "record_click_source_invalid",
        "{}",
        source.message
    );

    let follow = recording
        .mark(MarkRequest {
            click: Some(ClickSpec {
                follow: Some("value/count".to_owned()),
                ..ClickSpec::default()
            }),
            replace_click: true,
            ..recording.on_step(1)
        })
        .expect_err("a click follows a template only");
    assert_eq!(
        follow.code, "record_click_follow_invalid",
        "{}",
        follow.message
    );
    assert_eq!(
        follow.details.expect("details")["outcome"]["values"]["follow_problem"],
        "not_template"
    );

    let taken = recording
        .mark(step_two(vec![
            template("ui/next", NEXT),
            MarkSpec::region_only(
                "value/count/left".to_owned(),
                MarkFamily::Color,
                rect(70, 50, 4, 4),
            ),
        ]))
        .expect_err("the id is step 1's side target");
    assert_eq!(taken.code, "record_mark_id_conflict", "{}", taken.message);
    let side_taken = recording
        .mark(step_two(vec![
            MarkSpec::region_only(
                "value/other/left".to_owned(),
                MarkFamily::Color,
                rect(70, 50, 4, 4),
            ),
            value_test(
                "value/other",
                side(LEFT, decimal()),
                "gt",
                Some(json!({"const": 0})),
            ),
        ]))
        .expect_err("the side target id is another mark's");
    assert_eq!(
        side_taken.code, "record_mark_id_conflict",
        "{}",
        side_taken.message
    );
    assert_eq!(
        side_taken.details,
        Some(json!({"id": "value/other", "target_id": "value/other/left"}))
    );

    let mut left = side(LEFT, decimal());
    left["mul"] = json!(1);
    let mul = recording
        .mark(step_two(vec![
            template("ui/next", NEXT),
            value_test("value/other", left, "gt", Some(json!({"const": 0}))),
        ]))
        .expect_err("mul belongs to the right side");
    assert_eq!(mul.code, "validation_failed", "{}", mul.message);

    assert_eq!(recording.recording_bytes(), before, "nothing is written");
}
