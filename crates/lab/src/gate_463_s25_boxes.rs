// SPDX-License-Identifier: AGPL-3.0-only

// #463 S2-5 (contracts/lab-recording.md, "Drawing a box"; register §1.427; the design of board
// #471, its slices B1 and B2, ported): a `record mark` request draws boxes on the step's primary
// frame by two opposite corners. A recognize box becomes a template mark named
// `box/<nn>/<x>_<y>_<w>_<h>` (the step and the box), cut from the primary frame; a press box
// becomes the click's rectangle; after that everything is `record mark`'s own code, so a request
// with boxes records what the same marks and click written by hand record. The base images of a
// recognize box are the frames of the action that drew it: a sample of the request on which every
// recognize box reaches the recording's threshold (0.95, the template score the self-test and the
// run compute) is admitted as `--sample` admits one; a sample on which one does not is another
// state of the screen, not added, listed with its scores and not stored.
//
// Frames are synthetic: 96x64 pixels of flat gray (60) with 12x12 squares of gray noise, each of
// its own seed. There is no OCR model and no device; a one-call step presses through a fake
// Runtime. The tests sleep nowhere and read no clock. Every expected value comes from the work
// order's and the design's lines and the values above.
// - `s25_a_box_request_records_what_its_hand_written_marks_and_click_record`: two steps marked by
//   boxes answer each box {use, corners, rect, mark} and record the step table and the package the
//   same add and click written by hand record, but for the times.
// - `s25_corners_in_any_order_give_one_box_and_one_mark_name`: four orders, one rectangle and one
//   mark; the same box again is record_mark_id_conflict, nothing written.
// - `s25_a_box_outside_the_frame_is_refused`: region_outside_frame; a press box validation_failed.
// - `s25_a_flat_box_is_refused_box_flat_and_nothing_is_written`: box_flat with instead color.
// - `s25_each_rule_of_the_boxes_is_refused_with_its_word`: repeated, conflicting, beside an
//   application operation, a select, an ending and an inserted optional step.
// - `s25_one_call_with_boxes_presses_inside_the_press_box`: do --capture --record with boxes; the
//   same call again on a step whose press was not performed is the repeat refusal.
// - `s25_a_request_without_boxes_reads_and_answers_as_before`: no `boxes` member anywhere.
// - `s25_samples_of_the_request_that_every_box_holds_on_are_added_and_one_below_is_not`: own,
//   added and not_added with below_threshold; the package equals the hand-written one with the
//   fitting sample alone.
// - `s25_a_frame_the_step_held_before_the_call_refuses_a_box_below_it`: self_test_failed there.
// - `s25_with_no_other_frame_that_qualifies_the_step_keeps_the_frame_the_box_is_drawn_on`: a dry
//   run and the call answer the same; one frame, single_sample.

use crate::codes::{EndingProblem, LabCode, OptionalAfterProblem, SelectProblem};
use crate::recording::store::{hex_sha256, lab_dir, old_record_path, recording_path};
use crate::{
    AttachFrameRequest, ClickEffect, ClickPlan, CommitClickRequest, LAB_RECORD_MARK_SCHEMA, LabOcr,
    MarkFamily, MarkOutcome, MarkRequest, RecordRect, RecordStartOptions, RecordStepOutcome,
    RecordStepPorts, RecordStepRequest, RecordStopOptions, RecordingLock, SelfTestStatus,
    parse_mark_request, record_attach_frame, record_mark, record_plan_step, record_start,
    record_stop,
};
use actingcommand_contract::{LabError, LabResult};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_s25_boxes";
const TASK_ID: &str = "neutral_task";
const WIDTH: u32 = 96;
const HEIGHT: u32 = 64;
const BACKGROUND: u8 = 60;
const FIRST_SEED: u64 = 0x0463_2501;
const SECOND_SEED: u64 = 0x0463_2502;
// Other seeds of the noise: another state of the second square, of the first square, a square
// elsewhere on the first screen, and the next screen's button.
const CHANGED_SEED: u64 = 0x0463_2503;
const EARLIER_SEED: u64 = 0x0463_2505;
const ELSEWHERE_SEED: u64 = 0x0463_2510;
const STEP_2_SEED: u64 = 0x0463_2504;
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

const fn rect(x: i32, y: i32, width: i32, height: i32) -> RecordRect {
    RecordRect {
        x,
        y,
        width,
        height,
    }
}

// The first screen's button and the second screen's.
const BUTTON: RecordRect = rect(20, 20, 12, 12);
const NEXT: RecordRect = rect(60, 30, 12, 12);
const ELSEWHERE: RecordRect = rect(2, 50, 8, 8);
const STEP_2_BUTTON: RecordRect = rect(40, 44, 12, 12);
// The marks their recognize boxes become on steps 1 and 2.
const BUTTON_MARK: &str = "box/01/20_20_12_12";
const NEXT_MARK: &str = "box/02/60_30_12_12";
const BOX_1_MARK: &str = "box/01/60_30_12_12";
const STEP_2_MARK: &str = "box/02/40_44_12_12";

// A deterministic generator of neutral gray noise.
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

// The PNG of a flat gray screen with a square of noise of its own seed in each of `patches`.
fn screen(patches: &[(RecordRect, u64)]) -> Vec<u8> {
    let width = WIDTH as usize;
    let mut values = vec![BACKGROUND; width * HEIGHT as usize];
    for (area, seed) in patches {
        let mut noise = Noise(*seed);
        for row in area.y..area.y + area.height {
            for column in area.x..area.x + area.width {
                values[row as usize * width + column as usize] = noise.between(1, 255);
            }
        }
    }
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

fn first() -> Vec<u8> {
    screen(&[(BUTTON, FIRST_SEED)])
}

fn second() -> Vec<u8> {
    screen(&[(NEXT, SECOND_SEED)])
}

// The first screen with both squares.
fn both() -> Vec<u8> {
    screen(&[(BUTTON, FIRST_SEED), (NEXT, SECOND_SEED)])
}

// The same state of that screen with a square of its own elsewhere: both boxes hold on it.
fn fitting() -> Vec<u8> {
    screen(&[
        (BUTTON, FIRST_SEED),
        (NEXT, SECOND_SEED),
        (ELSEWHERE, ELSEWHERE_SEED),
    ])
}

// Another state of that screen: the second square shows other noise, the first the same.
fn half() -> Vec<u8> {
    screen(&[(BUTTON, FIRST_SEED), (NEXT, CHANGED_SEED)])
}

fn step_2() -> Vec<u8> {
    screen(&[(STEP_2_BUTTON, STEP_2_SEED)])
}

// A box of `usage` drawn from the corner `from` to the corner `to`.
fn boxed(usage: &str, from: (i32, i32), to: (i32, i32)) -> Value {
    json!({"use": usage, "corners": [{"x": from.0, "y": from.1}, {"x": to.0, "y": to.1}]})
}

// The first screen's button drawn from its top-left pixel to its bottom-right one.
fn button(usage: &str) -> Value {
    boxed(usage, (20, 20), (31, 31))
}

fn box_1() -> Value {
    boxed("recognize", (60, 30), (71, 41))
}

fn template(id: &str, area: RecordRect) -> Value {
    json!({"id": id, "family": "template", "region": area})
}

fn request(mut value: Value) -> MarkRequest {
    value["schema_version"] = json!(LAB_RECORD_MARK_SCHEMA);
    serde_json::from_value(value).expect("a record mark request")
}

fn score(frame: &Value) -> f64 {
    frame["score"]
        .as_f64()
        .unwrap_or_else(|| panic!("no score in {frame}"))
}

// `value` without its times and with `temp` in its texts replaced.
fn scrub(value: &mut Value, temp: &str) {
    match value {
        Value::Object(members) => {
            members
                .retain(|key, _| !key.ends_with("_unix_ms") || key == "record_started_at_unix_ms");
            for member in members.values_mut() {
                scrub(member, temp);
            }
        }
        Value::Array(items) => {
            for item in items {
                scrub(item, temp);
            }
        }
        Value::String(content) => *content = content.replace(temp, "<temp>"),
        _ => {}
    }
}

// Every file under `dir`, by its `/`-separated path relative to it, with its bytes, in name order.
fn files_under(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    let mut pending = vec![(dir.to_path_buf(), String::new())];
    while let Some((path, prefix)) = pending.pop() {
        for entry in fs::read_dir(&path).expect("directory") {
            let entry = entry.expect("entry");
            let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
            if entry.file_type().expect("file type").is_dir() {
                pending.push((entry.path(), format!("{name}/")));
            } else {
                files.push((name, fs::read(entry.path()).expect("file")));
            }
        }
    }
    files.sort();
    files
}

// The files of the package `record stop` wrote, the task's provenance without its generation time
// (the time the recording was last written).
fn package(lab: &Value) -> BTreeMap<String, Vec<u8>> {
    let bytes = fs::read(lab["path"].as_str().expect("package path")).expect("package");
    let mut entries =
        expand_content_container(&bytes, ContentContainer::Zip, ContainmentLimits::default())
            .expect("package container");
    for (name, bytes) in entries.iter_mut() {
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

// A state directory with an active session of `INSTANCE` and its Lab recording, before any step,
// without an OCR model. The lock is dropped before the directory.
struct Recording {
    lock: RecordingLock,
    ocr: LabOcr,
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
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_463_s25", "Lab-CLI")
            .expect("recording lock");
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
            ocr: LabOcr::from_vision_root(&temp.path().join("no-vision")),
            temp,
        }
    }

    // Writes `bytes` as the frame file `name` on the caller's side and returns its path.
    fn frame(&self, name: &str, bytes: &[u8]) -> String {
        let path = self.temp.path().join(format!("{name}.png"));
        fs::write(&path, bytes).expect("frame file");
        path.display().to_string()
    }

    fn mark(&self, value: Value) -> Result<MarkOutcome, LabError> {
        record_mark(&self.lock, &request(value), false, &self.ocr)
    }

    fn dry(&self, value: Value) -> Result<MarkOutcome, LabError> {
        record_mark(&self.lock, &request(value), true, &self.ocr)
    }

    // `record mark` of `value`, which must stand.
    fn marked(&self, value: Value) -> MarkOutcome {
        self.mark(value)
            .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details))
    }

    // `capture --record` of the screen `bytes`.
    fn capture(&self, bytes: Vec<u8>) {
        record_attach_frame(
            &self.lock,
            AttachFrameRequest {
                png: bytes,
                source: "lab_capture".to_owned(),
                runtime_artifact: None,
                capture_backend: Some("adb_screencap".to_owned()),
                freshness: None,
            },
        )
        .unwrap_or_else(|error| panic!("capture --record: {}: {}", error.code, error.message));
    }

    // The one call `do --capture --record` with the mark request `value`, through `runtime`.
    fn one_call(&self, value: Value, runtime: &mut FakeRuntime) -> LabResult<RecordStepOutcome> {
        let step = RecordStepRequest {
            mark: Some(request(value)),
            ..RecordStepRequest::default()
        };
        record_plan_step(&self.lock, &step, &self.ocr)?.perform(&self.lock, runtime)
    }

    // `record stop`: the `lab` output with its status.
    fn stop(&self) -> Value {
        let outcome = record_stop(&self.lock, &RecordStopOptions::default(), &self.ocr)
            .unwrap_or_else(|error| panic!("record stop: {} {:?}", error.code, error.details));
        let mut lab = serde_json::to_value(&outcome.lab).expect("lab JSON");
        lab["lab_status"] = json!(outcome.lab_status);
        lab
    }

    fn state(&self) -> std::path::PathBuf {
        self.temp.path().join("state")
    }

    fn recording(&self) -> Value {
        serde_json::from_slice(
            &fs::read(recording_path(&self.state(), RECORD_ID)).expect("recording file"),
        )
        .expect("recording JSON")
    }

    // The step table without its times and the temporary directory.
    fn normalised(&self) -> Value {
        let mut recording = self.recording();
        scrub(&mut recording, &self.temp.path().display().to_string());
        recording
    }

    fn files(&self) -> Vec<(String, Vec<u8>)> {
        files_under(&lab_dir(&self.state(), RECORD_ID))
    }
}

/// What the fake Runtime answers to the press.
#[derive(Clone, Copy, Debug)]
enum Answer {
    Performed,
    NotPerformed,
}

/// The fake Runtime of one call: what it answers and what it was asked, in order.
struct FakeRuntime {
    answer: Answer,
    seen: Vec<String>,
}

impl FakeRuntime {
    fn new(answer: Answer) -> Self {
        Self {
            answer,
            seen: Vec::new(),
        }
    }
}

impl RecordStepPorts for FakeRuntime {
    fn press(&mut self, plan: &ClickPlan) -> LabResult<CommitClickRequest> {
        self.seen.push(format!("press {}", plan.step));
        match self.answer {
            Answer::Performed => Ok(CommitClickRequest {
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
            }),
            Answer::NotPerformed => Err(LabError::device(
                "the fake Runtime did not perform the input",
            )),
        }
    }

    fn wait(&mut self, duration: Duration) {
        self.seen.push(format!("wait {}", duration.as_millis()));
    }

    fn capture(&mut self) -> LabResult<AttachFrameRequest> {
        self.seen.push("capture".to_owned());
        Err(LabError::device("the fake Runtime takes no frame"))
    }
}

fn inside(x: i32, y: i32, area: RecordRect) -> bool {
    x >= area.x && y >= area.y && x < area.x + area.width && y < area.y + area.height
}

// The details of a `validation_failed` refusal of the boxes with `problem`.
fn boxes_refusal(error: LabError, problem: &str) -> Value {
    assert_eq!(error.code, "validation_failed", "{}", error.message);
    let details = error.details.expect("details");
    assert_eq!(details["argument"], "boxes", "{details}");
    assert_eq!(details["argument_problem"], problem, "{details}");
    details
}

// The design's test (1) of B1: the boxes of two steps record the step table and generate the
// package that the same template marks and click written by hand record and generate, but for the
// times; the answer gives each box with its rectangle and the mark it became.
#[test]
fn s25_a_box_request_records_what_its_hand_written_marks_and_click_record() {
    let boxes = Recording::new();
    let marked = boxes.marked(json!({
        "frame": boxes.frame("first", &first()),
        "boxes": [button("recognize"), boxed("press", (31, 31), (20, 20))]
    }));

    let views = serde_json::to_value(&marked.boxes).expect("boxes JSON");
    assert_eq!(views[0]["use"], "recognize", "{views}");
    assert_eq!(
        views[0]["corners"],
        json!([{"x": 20, "y": 20}, {"x": 31, "y": 31}]),
        "{views}"
    );
    assert_eq!(views[0]["rect"], json!(BUTTON), "{views}");
    assert_eq!(views[0]["mark"], BUTTON_MARK, "{views}");
    assert_eq!(
        views[1],
        json!({"use": "press", "corners": [{"x": 31, "y": 31}, {"x": 20, "y": 20}],
               "rect": BUTTON}),
        "a press box is its rectangle"
    );
    let mark = &marked.marks[0];
    assert_eq!(mark.id, BUTTON_MARK);
    assert_eq!(mark.family, MarkFamily::Template);
    assert_eq!(mark.region, Some(BUTTON));
    assert_eq!(
        mark.crop.as_ref().map(|crop| crop.asset.as_str()),
        Some("assets/box_01_20_20_12_12.png")
    );
    assert_eq!(mark.self_test.status, SelfTestStatus::Passed);
    let click = marked.click.as_ref().expect("the click");
    assert_eq!(click.rect, BUTTON);
    assert_eq!(click.source, "declared");
    boxes.marked(json!({
        "frame": boxes.frame("second", &second()),
        "boxes": [boxed("recognize", (60, 30), (71, 41))]
    }));

    let by_hand = Recording::new();
    by_hand.marked(json!({
        "frame": by_hand.frame("first", &first()),
        "add": [template(BUTTON_MARK, BUTTON)],
        "click": {"region": BUTTON}
    }));
    by_hand.marked(json!({
        "frame": by_hand.frame("second", &second()),
        "add": [template(NEXT_MARK, NEXT)]
    }));
    assert_eq!(boxes.normalised(), by_hand.normalised());

    let from_boxes = boxes.stop();
    let written = by_hand.stop();
    assert_eq!(from_boxes["lab_status"], "generated", "{from_boxes}");
    assert_eq!(written["lab_status"], "generated", "{written}");
    assert_eq!(package(&from_boxes), package(&written));
}

// The design's test (2) of B1: the two corners in any order give one rectangle and one mark name,
// and the same box sent again on the same step is that mark, `record_mark_id_conflict` naming it,
// nothing written.
#[test]
fn s25_corners_in_any_order_give_one_box_and_one_mark_name() {
    let recording = Recording::new();
    let frame = recording.frame("first", &first());
    for (from, to) in [
        ((20, 20), (31, 31)),
        ((31, 31), (20, 20)),
        ((31, 20), (20, 31)),
        ((20, 31), (31, 20)),
    ] {
        let tried = recording
            .dry(json!({"frame": frame, "boxes": [boxed("recognize", from, to)]}))
            .unwrap_or_else(|error| panic!("dry run: {} {:?}", error.code, error.details));
        assert_eq!(tried.status, "marks_validated");
        assert_eq!(tried.marks[0].id, BUTTON_MARK, "{from:?} {to:?}");
        assert_eq!(tried.boxes[0].rect, BUTTON, "{from:?} {to:?}");
    }
    let marked = recording.marked(json!({"frame": frame, "boxes": [button("recognize")]}));
    assert_eq!(marked.marks[0].id, BUTTON_MARK);
    assert_eq!(marked.step, Some(1));

    let before = recording.files();
    let again = recording
        .mark(json!({"frame": frame, "boxes": [boxed("recognize", (31, 31), (20, 20))]}))
        .expect_err("the same box again");
    assert_eq!(again.code, "record_mark_id_conflict", "{}", again.message);
    assert_eq!(again.details.expect("details")["id"], BUTTON_MARK);
    assert_eq!(recording.files(), before, "a file changed");
}

// The design's test (3) of B1: a recognize box outside the frame is its mark's
// `region_outside_frame`; a press box outside it is the click's `validation_failed`; nothing is
// written.
#[test]
fn s25_a_box_outside_the_frame_is_refused() {
    let recording = Recording::new();
    let frame = recording.frame("first", &first());
    let before = recording.files();

    let error = recording
        .mark(json!({"frame": frame, "boxes": [boxed("recognize", (90, 60), (100, 70))]}))
        .expect_err("a recognize box outside the frame");
    assert_eq!(error.code, "record_mark_rejected", "{}", error.message);
    let details = error.details.expect("details");
    let marks = &details["marks"];
    assert_eq!(marks[0]["id"], "box/01/90_60_11_11", "{marks}");
    assert_eq!(marks[0]["reason"], "region_outside_frame", "{marks}");

    let error = recording
        .mark(json!({"frame": frame, "boxes": [boxed("press", (90, 60), (100, 70))]}))
        .expect_err("a press box outside the frame");
    assert_eq!(error.code, "validation_failed", "{}", error.message);
    assert_eq!(recording.files(), before, "a file changed");
}

// The design's test (4) of B1: a recognize box of one gray value is refused `box_flat` with
// `instead: "color"`, that one reason only, the box's index and rectangle, beside a box that
// holds; nothing is written.
#[test]
fn s25_a_flat_box_is_refused_box_flat_and_nothing_is_written() {
    let recording = Recording::new();
    let before = recording.files();
    let error = recording
        .mark(json!({
            "frame": recording.frame("first", &first()),
            "boxes": [button("recognize"), boxed("recognize", (2, 40), (9, 47))]
        }))
        .expect_err("a flat box");
    assert_eq!(error.code, "record_mark_rejected", "{}", error.message);
    assert_eq!(
        error.details.expect("details")["marks"],
        json!([{"id": "box/01/2_40_8_8", "reason": "box_flat", "box": 1,
                "rect": rect(2, 40, 8, 8), "instead": "color"}])
    );
    assert_eq!(recording.files(), before, "a file changed");
}

// The design's tests (5) of B1: two press boxes and the same recognize box twice are `repeated`; a
// press box beside a click, and boxes beside a transition or a step operation, `conflicting`; a
// press box beside an application operation is `record_application_with_click`, beside a select
// `record_select_invalid` `with_click`; boxes beside an ending are the ending's
// `with_other_change`, and (this slice, on the parent's main) beside an inserted optional step the
// insertion's `with_other_change`. Nothing is written.
#[test]
fn s25_each_rule_of_the_boxes_is_refused_with_its_word() {
    let recording = Recording::new();
    let frame = recording.frame("first", &first());
    recording.marked(
        json!({"frame": frame, "add": [{"id": "state/back", "family": "color",
                                                       "region": rect(2, 40, 8, 8)}]}),
    );
    let before = recording.files();
    let press = button("press");

    let details = boxes_refusal(
        recording
            .mark(json!({"boxes": [press, boxed("press", (60, 30), (71, 41))]}))
            .expect_err("two press boxes"),
        "repeated",
    );
    assert_eq!(details["box"], 1, "{details}");
    let details = boxes_refusal(
        recording
            .mark(json!({"boxes": [button("recognize"), boxed("recognize", (31, 31), (20, 20))]}))
            .expect_err("the same recognize box twice"),
        "repeated",
    );
    assert_eq!(details["box"], 1, "{details}");
    let details = boxes_refusal(
        recording
            .mark(json!({"boxes": [press], "click": {"region": BUTTON}}))
            .expect_err("a press box beside a click"),
        "conflicting",
    );
    assert_eq!(details["member"], "click", "{details}");
    for (member, beside) in [
        (
            "transition",
            json!({"step": 1, "transition": {"kind": "none"}}),
        ),
        (
            "step_action",
            json!({"step_action": {"kind": "drop_step", "step": 1}}),
        ),
    ] {
        let mut value = beside;
        value["boxes"] = json!([button("recognize")]);
        let details = boxes_refusal(recording.mark(value).expect_err(member), "conflicting");
        assert_eq!(details["member"], member, "{details}");
    }

    let error = recording
        .mark(json!({"boxes": [press], "application": {"action": "launch"}}))
        .expect_err("a press box beside an application operation");
    assert_eq!(
        error.code, "record_application_with_click",
        "{}",
        error.message
    );

    let select = json!({
        "slots": [{"rect": rect(0, 0, 40, 20), "name": rect(0, 0, 40, 20)}],
        "read": {"languages": ["en"], "timeout_ms": 1000, "minimum_confidence_milli": 800,
                 "model_ref": "neutral-ocr", "model_sha256": MODEL_SHA256},
        "names": [{"id": "item_a", "aliases": ["ALPHA"]}],
        "match": {"max_distance": 1, "minimum_margin": 1},
        "wanted": {"name": "list_item", "default": "item_a"}
    });
    let error = recording
        .mark(json!({"boxes": [press], "select": select}))
        .expect_err("a press box beside a select");
    assert_eq!(
        error.code,
        LabCode::RecordSelectInvalid.as_str(),
        "{}",
        error.message
    );
    assert_eq!(
        error.details.expect("details")["select_problem"],
        SelectProblem::WithClick.as_str()
    );

    let error = recording
        .mark(json!({
            "ending": {"frame": frame, "outcome_key": "neutral_task_short",
                       "add": [{"id": "ending/short", "family": "color",
                                "region": rect(2, 40, 8, 8)}]},
            "boxes": [button("recognize")]
        }))
        .expect_err("boxes beside an ending");
    assert_eq!(
        error.code,
        LabCode::RecordEndingInvalid.as_str(),
        "{}",
        error.message
    );
    let details = error.details.expect("details");
    assert_eq!(
        details["ending_problem"],
        EndingProblem::WithOtherChange.as_str(),
        "{details}"
    );
    assert_eq!(details["members"], json!(["boxes"]), "{details}");

    let error = recording
        .mark(json!({
            "optional_after": {"step": 1, "frame": frame,
                               "add": [{"id": "popup/notice", "family": "color",
                                        "region": rect(2, 40, 8, 8)}],
                               "click": {"region": BUTTON}},
            "boxes": [button("recognize")]
        }))
        .expect_err("boxes beside an inserted optional step");
    assert_eq!(
        error.code,
        LabCode::RecordOptionalAfterInvalid.as_str(),
        "{}",
        error.message
    );
    let details = error.details.expect("details");
    assert_eq!(
        details["optional_after_problem"],
        OptionalAfterProblem::WithOtherChange.as_str(),
        "{details}"
    );
    assert_eq!(details["members"], json!(["boxes"]), "{details}");
    assert_eq!(recording.files(), before, "a file changed");
}

// The design's test (6) of B1: `do --capture --record` with a mark request of a recognize and a
// press box marks the open step and presses inside the press box; on a step whose press was not
// performed the same call again is `record mark`'s `record_mark_id_conflict` with the message of a
// step that holds its marks and no outcome of a press, and nothing is pressed or written.
#[test]
fn s25_one_call_with_boxes_presses_inside_the_press_box() {
    let pressed = Recording::new();
    pressed.capture(first());
    let mut runtime = FakeRuntime::new(Answer::Performed);
    let outcome = pressed
        .one_call(
            json!({"boxes": [button("recognize"), button("press")]}),
            &mut runtime,
        )
        .unwrap_or_else(|error| panic!("one call: {}: {:?}", error.code, error.details));
    assert_eq!(runtime.seen, ["press 1"]);
    let mark = outcome.mark.expect("the mark output");
    assert_eq!(mark.marks[0].id, BUTTON_MARK);
    assert_eq!(mark.boxes.len(), 2);
    let execution = outcome.click.click.execution.expect("the press");
    assert!(
        inside(execution.point.x, execution.point.y, BUTTON),
        "{:?}",
        execution.point
    );
    assert_eq!(outcome.click.click.rect, BUTTON);
    assert!(outcome.click.step_closed);

    let unpressed = Recording::new();
    unpressed.capture(first());
    let call = json!({"boxes": [button("recognize"), button("press")]});
    let mut refused = FakeRuntime::new(Answer::NotPerformed);
    let error = unpressed
        .one_call(call.clone(), &mut refused)
        .expect_err("the press not performed");
    assert_eq!(error.code, LabError::device("").code, "{}", error.message);
    let step = unpressed.recording()["steps"][0].clone();
    assert_eq!(step["marks"][0]["id"], BUTTON_MARK, "{step}");
    assert_eq!(step["click"]["rect"], json!(BUTTON), "{step}");

    let before = unpressed.files();
    let mut again = FakeRuntime::new(Answer::Performed);
    let error = unpressed
        .one_call(call, &mut again)
        .expect_err("the same call again");
    assert_eq!(error.code, "record_mark_id_conflict", "{}", error.message);
    for words in ["its marks and its click", "no outcome of a press"] {
        assert!(error.message.contains(words), "{words}: {}", error.message);
    }
    assert!(again.seen.is_empty(), "{:?}", again.seen);
    assert_eq!(unpressed.files(), before, "a file changed");
}

// The design's test (9) of B1: a request without `boxes` decodes and encodes with no `boxes`
// member, and its answer has none.
#[test]
fn s25_a_request_without_boxes_reads_and_answers_as_before() {
    let line = json!({"schema_version": LAB_RECORD_MARK_SCHEMA, "page": "home"}).to_string();
    let parsed = parse_mark_request(&line)
        .unwrap_or_else(|error| panic!("the request: {}: {}", error.code, error.message));
    assert!(parsed.boxes.is_empty());
    let encoded = serde_json::to_value(&parsed).expect("request JSON");
    assert!(encoded.get("boxes").is_none(), "{encoded}");

    let recording = Recording::new();
    let marked = recording.marked(json!({
        "frame": recording.frame("first", &first()),
        "add": [{"id": "ui/button", "family": "template", "region": BUTTON}],
        "click": {"region": BUTTON}
    }));
    let answer = serde_json::to_value(&marked).expect("answer JSON");
    assert!(answer.get("boxes").is_none(), "{answer}");
}

// B2's test (1), with the work order's words: the samples of the request on which every recognize
// box reaches the threshold are added, one on which a box is below it is listed with each box's
// score and not stored (no frame of the step, no file), its `because` naming the box; the step's
// frames are the frame the boxes are drawn on (`own`) and the added sample, and the recording and
// its package are those of the same marks and click written by hand with `--sample` of the fitting
// frame alone.
#[test]
fn s25_samples_of_the_request_that_every_box_holds_on_are_added_and_one_below_is_not() {
    let boxes = Recording::new();
    let marked = boxes.marked(json!({
        "frame": boxes.frame("both", &both()),
        "samples": [boxes.frame("fitting", &fitting()), boxes.frame("half", &half())],
        "boxes": [button("recognize"), box_1(), button("press")]
    }));

    assert_eq!(marked.samples.len(), 1, "{:?}", marked.samples);
    assert_eq!(marked.samples[0].frame_id, "f0002");
    assert_eq!(marked.samples[0].sha256, hex_sha256(&fitting()));
    assert_eq!(
        marked.step_state.as_ref().map(|state| state.frames),
        Some(2)
    );
    for mark in &marked.marks {
        assert_eq!(mark.self_test.status, SelfTestStatus::Passed, "{}", mark.id);
        assert_eq!(mark.self_test.frames, 2, "{}", mark.id);
    }
    let views = serde_json::to_value(&marked.boxes).expect("boxes JSON");
    let because = json!({"reason": "below_threshold", "mark": BOX_1_MARK, "box": 1});
    for (index, (mark, area)) in [(BUTTON_MARK, BUTTON), (BOX_1_MARK, NEXT)]
        .into_iter()
        .enumerate()
    {
        let view = &views[index];
        assert_eq!(view["mark"], mark, "{view}");
        assert_eq!(view["threshold"], 0.95, "{view}");
        assert_eq!(view["metric"], "ccoeff_normed", "{view}");
        let frames = view["frames"].as_array().expect("frames");
        let taken = frames
            .iter()
            .map(|frame| frame["taken"].clone())
            .collect::<Vec<_>>();
        assert_eq!(taken, ["own", "added", "not_added"], "{view}");
        assert_eq!(frames[0]["frame_id"], "f0001", "{view}");
        assert_eq!(frames[0]["matched_rect"], json!(area), "{view}");
        assert!(frames[0].get("sample").is_none(), "{view}");
        assert_eq!(frames[1]["frame_id"], "f0002", "{view}");
        assert_eq!(frames[1]["sample"], 0, "{view}");
        assert!(frames[2].get("frame_id").is_none(), "{view}");
        assert_eq!(frames[2]["sample"], 1, "{view}");
        assert_eq!(frames[2]["because"], because, "{view}");
        for frame in &frames[..2] {
            assert!(score(frame) >= 0.95, "{view}");
            assert!(frame.get("because").is_none(), "{view}");
        }
    }
    assert!(score(&views[0]["frames"][2]) >= 0.95, "{views}");
    assert!(score(&views[1]["frames"][2]) < 0.95, "{views}");
    assert_eq!(
        views[2],
        json!({"use": "press", "corners": [{"x": 20, "y": 20}, {"x": 31, "y": 31}],
               "rect": BUTTON}),
        "a press box has no frames"
    );
    let stored = boxes.recording();
    let frame_ids = stored["steps"][0]["frames"]
        .as_array()
        .expect("frames")
        .iter()
        .map(|frame| frame["frame_id"].clone())
        .collect::<Vec<_>>();
    assert_eq!(frame_ids, ["f0001", "f0002"], "{stored}");
    let left_sha256 = hex_sha256(&half());
    assert!(
        !stored.to_string().contains(&left_sha256),
        "the frame not added is in the recording"
    );
    assert!(
        !lab_dir(&boxes.state(), RECORD_ID)
            .join("frames")
            .join(format!("{left_sha256}.png"))
            .exists(),
        "the frame not added is stored"
    );

    let by_hand = Recording::new();
    by_hand.marked(json!({
        "frame": by_hand.frame("both", &both()),
        "samples": [by_hand.frame("fitting", &fitting())],
        "add": [template(BUTTON_MARK, BUTTON), template(BOX_1_MARK, NEXT)],
        "click": {"region": BUTTON}
    }));
    boxes.marked(json!({
        "frame": boxes.frame("step_2", &step_2()),
        "boxes": [boxed("recognize", (40, 44), (51, 55))]
    }));
    by_hand.marked(json!({
        "frame": by_hand.frame("step_2", &step_2()),
        "add": [template(STEP_2_MARK, STEP_2_BUTTON)]
    }));
    assert_eq!(boxes.normalised(), by_hand.normalised());
    let from_boxes = boxes.stop();
    let written = by_hand.stop();
    assert_eq!(from_boxes["lab_status"], "generated", "{from_boxes}");
    assert_eq!(package(&from_boxes), package(&written));
}

// B2's test (2): a frame the step admitted before the call is the step's state already, so a
// recognize box below the threshold there refuses the request (`self_test_failed` with that
// frame) and nothing is written.
#[test]
fn s25_a_frame_the_step_held_before_the_call_refuses_a_box_below_it() {
    let recording = Recording::new();
    recording.marked(json!({
        "frame": recording.frame("first", &first()),
        "samples": [recording.frame("earlier", &screen(&[(BUTTON, EARLIER_SEED)]))],
        "add": [{"id": "state/back", "family": "color", "region": rect(2, 40, 8, 8)}]
    }));
    let before = recording.files();

    let error = recording
        .mark(json!({"boxes": [button("recognize")]}))
        .expect_err("a box below the threshold on a frame the step held");
    assert_eq!(error.code, "record_mark_rejected", "{}", error.message);
    let details = error.details.expect("details");
    let marks = details["marks"].as_array().expect("marks");
    assert_eq!(marks.len(), 1, "{details}");
    assert_eq!(marks[0]["id"], BUTTON_MARK, "{details}");
    assert_eq!(marks[0]["reason"], "self_test_failed", "{details}");
    assert_eq!(marks[0]["frame_id"], "f0002", "{details}");
    assert_eq!(recording.files(), before, "a file changed");
}

// B2's test (3): the frame the box is drawn on always qualifies; when no other frame of the action
// does, the step has that one frame and the answer says so (`step_state.frames` 1, the mark's
// `single_sample`, the sample not added with its score). A dry run answers the same and writes
// nothing.
#[test]
fn s25_with_no_other_frame_that_qualifies_the_step_keeps_the_frame_the_box_is_drawn_on() {
    let recording = Recording::new();
    let request = json!({
        "frame": recording.frame("first", &first()),
        "samples": [recording.frame("moving", &screen(&[(BUTTON, EARLIER_SEED)]))],
        "boxes": [button("recognize")]
    });
    let before = recording.files();
    let tried = recording
        .dry(request.clone())
        .unwrap_or_else(|error| panic!("dry run: {} {:?}", error.code, error.details));
    assert_eq!(recording.files(), before, "the dry run wrote");
    assert_eq!(tried.status, "marks_validated");

    let marked = recording.marked(request);
    assert_eq!(marked.boxes, tried.boxes);
    for outcome in [&tried, &marked] {
        assert!(outcome.samples.is_empty(), "{:?}", outcome.samples);
        assert_eq!(
            outcome.step_state.as_ref().map(|state| state.frames),
            Some(1)
        );
        assert!(outcome.marks[0].self_test.single_sample);
    }
    let views = serde_json::to_value(&marked.boxes).expect("boxes JSON");
    let frames = views[0]["frames"].as_array().expect("frames");
    assert_eq!(frames.len(), 2, "{views}");
    assert_eq!(frames[0]["taken"], "own", "{views}");
    assert_eq!(frames[0]["frame_id"], "f0001", "{views}");
    assert!(score(&frames[0]) >= 0.95, "{views}");
    assert_eq!(frames[1]["taken"], "not_added", "{views}");
    assert_eq!(frames[1]["sample"], 0, "{views}");
    assert!(score(&frames[1]) < 0.95, "{views}");
    assert_eq!(
        frames[1]["because"],
        json!({"reason": "below_threshold", "mark": BUTTON_MARK, "box": 0}),
        "{views}"
    );
    assert_eq!(
        recording.recording()["steps"][0]["frames"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
}
