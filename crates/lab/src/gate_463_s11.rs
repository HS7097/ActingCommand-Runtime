// SPDX-License-Identifier: AGPL-3.0-only

// #463 S1-1 (contracts/lab-recording.md, "Recording while playing"; register §1.402): a press is
// recorded by the command that makes it. `record_press` is what `tap`, `swipe` and `long-tap
// --record` do under the recording lock: a frame first when no step is open, the press planned on
// the open step, the press sent once through the caller (a fake Runtime here), its outcome
// recorded, then the wait and the frame after it, on the fake's clock and capture
// (`RecordFramePorts`). Every capture arrives as `capture --record`'s does.
// - `s11_a_tap_on_a_new_recording_records_the_screens_before_and_after`: one capture opens step 1,
//   the input is `Tap{100, 200}`, step 1 declares `rect {100, 200, 1, 1}`, `source "point"`, its
//   execution names `before_frame` f0001; after the wait of 800 ms one capture opens step 2, which
//   is step 1's `after_frame` f0002, with `after_capture`; the answer names both frames.
// - `s11_a_long_press_is_a_long_press_of_one_pixel`: `LongTap` with 800 ms; the gesture
//   `long_press` 800 ms on the box of one pixel.
// - `s11_a_swipe_is_the_recorded_swipe_of_one_pixel_boxes`: `Swipe`; the drag between boxes of one
//   pixel, `source "swipe"`, no carrier package, both frames named.
// - `s11_an_uncertain_answer_is_indeterminate_and_takes_no_frame_after`: the execution
//   `indeterminate`, the step open, `record_click_indeterminate` with the press as recorded
//   (`press_indeterminate`), no wait and no capture after the press.
// - `s11_a_press_not_performed_leaves_every_file_as_it_was`: the input's own error, and every file
//   of the recording keeps its bytes.
// - `s11_a_refusal_before_the_press_presses_nothing`: a point outside the frame (`validation_failed`,
//   `outside_frame`), a long press of 0 ms (`duration_range`) and a wait over 60000 ms (`range`):
//   nothing pressed, nothing written.
// - `s11_a_step_that_declares_a_click_presses_nothing`: the open step's click declared by `record
//   mark`: `record_click_exists`, nothing captured, pressed or written.
// - `s11_a_frame_after_that_cannot_be_taken_leaves_the_press`: step 1 closed and performed, the
//   capture's own error whose details begin with `record`, then `after: {after_ms, taken: false}`.
// - `s11_marked_afterwards_the_recording_generates_a_one_pixel_click_the_kernel_admits`: marks added
//   with `record mark --step`; `record stop --dry-run` admits the package (the kernel's admission)
//   and `record stop` generates step 1's click as the `rect` of one pixel.
// - `s11_a_recording_without_such_a_press_has_no_new_member`: a press of `do --capture --record`
//   and the frame after it: no `before_frame`, `after_frame` or `after_capture` anywhere, the
//   execution's members those an earlier build wrote.
// Frames are synthetic 1280x720 (a square of one colour at the top left on gray); no Runtime, no
// device; the tests read no clock and never sleep.
// #463 S1-2 (register §1.403) adapts six of these tests to what it deliberately changes: the frame
// after a press is the capture that is still with the one before it, so the scripts give the
// screen after twice (a wait of 300 ms between) and `after_capture` is `{800, 5000, true, 1100, 2,
// 0}`; and an open step's frame stands only while it is younger than 1000 ms, so the two tests that
// press on such a step record its frame at a time the clock has not reached.

use crate::recording::store::{lab_dir, old_record_path, recording_path, save_recording};
use crate::{
    AfterCapture, AttachFrameRequest, ClickEffect, ClickPlan, ClickSpec, CommitClickRequest,
    LAB_RECORD_MARK_SCHEMA, LabOcr, LabRecording, MarkFamily, MarkRequest, MarkSpec, OpaqueJson,
    PlanClickRequest, PlannedSwipe, PointPress, RecordFramePorts, RecordPoint, RecordPressOutcome,
    RecordPressRequest, RecordRect, RecordStartOptions, RecordStopOptions, RecordingLock,
    record_attach_frame, record_commit_click, record_mark, record_plan_click, record_press,
    record_press_input, record_start, record_stop,
};
use actingcommand_contract::{InputAction, LabError, LabResult};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0463_s11";
const TASK_ID: &str = "neutral_task";
const STARTED_AT: u64 = 1_700_000_000_000;
const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
const GRAY: [u8; 3] = [60, 60, 60];
/// The colour of each screen's square.
const COLORS: [[u8; 3]; 3] = [[230, 20, 20], [20, 230, 20], [20, 20, 230]];
/// Each screen's square, a colour mark's region.
const SQUARE: RecordRect = rect(0, 0, 40, 40);
/// #463 S1-2: a time the clock has not reached (2100-01-01): a frame recorded then is younger than
/// any bound, so a press on its step takes no frame before it.
const UNREACHED_UNIX_MS: u64 = 4_102_444_800_000;

const fn rect(x: i32, y: i32, width: i32, height: i32) -> RecordRect {
    RecordRect {
        x,
        y,
        width,
        height,
    }
}

// The PNG of screen `index`: its square of colour at the top left on gray.
fn screen(index: usize) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            pixels.extend(if x < 40 && y < 40 {
                COLORS[index]
            } else {
                GRAY
            });
        }
    }
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

// Screen `index` as `capture --record` hands it to the recording.
fn captured(index: usize) -> AttachFrameRequest {
    AttachFrameRequest {
        png: screen(index),
        source: "lab_capture".to_owned(),
        runtime_artifact: None,
        capture_backend: Some("adb_screencap".to_owned()),
        freshness: Some(
            OpaqueJson::from_serializable(&json!({"required": false})).expect("freshness"),
        ),
    }
}

// The fake capture's own error.
fn capture_failure() -> LabError {
    LabError::device("the fake capture took no frame")
}

fn opaque(value: Value) -> Option<OpaqueJson> {
    Some(OpaqueJson::from_serializable(&value).expect("opaque value"))
}

/// What the fake Runtime answers to the input.
#[derive(Clone, Copy, Debug)]
enum Answer {
    Performed,
    Indeterminate,
    NotPerformed,
}

// What the command hands the recording for the input receipt of request `id`: no carrier package,
// no Runtime frames.
fn receipt(id: &str, effect: ClickEffect) -> CommitClickRequest {
    CommitClickRequest {
        effect,
        has_failure: false,
        carrier_package: None,
        req_id: opaque(json!(id)),
        correlation_id: opaque(json!(format!("{id}-correlation"))),
        action_id: match effect {
            ClickEffect::Performed => opaque(json!(format!("{id}-action"))),
            ClickEffect::Indeterminate => None,
        },
        lease_id: None,
        failure: None,
        before: None,
        after: None,
    }
}

/// The fake Runtime's capture and the caller's clock: the screens its captures take, in order
/// (`None`: that capture fails), and what it was asked, in the log it shares with the press.
struct FakeDevice<'a> {
    screens: VecDeque<Option<usize>>,
    log: &'a RefCell<Vec<String>>,
}

impl RecordFramePorts for FakeDevice<'_> {
    fn wait(&mut self, duration: Duration) {
        self.log
            .borrow_mut()
            .push(format!("wait {}", duration.as_millis()));
    }

    fn capture(&mut self) -> LabResult<AttachFrameRequest> {
        self.log.borrow_mut().push("capture".to_owned());
        match self.screens.pop_front() {
            Some(Some(index)) => Ok(captured(index)),
            Some(None) => Err(capture_failure()),
            None => panic!("a capture the test did not script"),
        }
    }
}

/// What one `record_press` call saw: its result, the log of the fake (`capture`, `press`,
/// `wait <ms>`) and the inputs it was given, in order.
struct Pressed {
    result: LabResult<RecordPressOutcome>,
    log: Vec<String>,
    inputs: Vec<InputAction>,
}

// A state directory with an active session of `INSTANCE` (the header `record start` writes) and its
// Lab recording, before any step. The lock is held as one command holds it and dropped before the
// directory.
struct Bench {
    lock: RecordingLock,
    ocr: LabOcr,
    temp: TempDir,
}

impl Bench {
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
                "started_at_unix_ms": STARTED_AT
            }))
            .expect("session JSON"),
        )
        .expect("session file");
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_463_s11", "Lab-CLI")
            .expect("recording lock");
        let ocr = LabOcr::from_vision_root(&temp.path().join("no-vision"));
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

    // `capture --record` of screen `index`.
    fn capture(&self, index: usize) {
        record_attach_frame(&self.lock, captured(index))
            .unwrap_or_else(|error| panic!("frame {index}: {}: {}", error.code, error.message));
    }

    // #463 S1-2: the live primary frame of step `step` as recorded at `at_unix_ms`, written as the
    // recording writes its step table: whether a press takes it again then depends on no clock.
    fn recorded_at(&self, step: u32, at_unix_ms: u64) {
        let state = self.state();
        let path = recording_path(&state, RECORD_ID);
        let mut recording: LabRecording =
            serde_json::from_slice(&fs::read(&path).expect("step table")).expect("step table JSON");
        let frame = recording
            .steps
            .iter_mut()
            .find(|item| item.index == step)
            .and_then(|item| {
                item.frames
                    .iter_mut()
                    .rev()
                    .find(|frame| frame.role == "primary" && !frame.superseded)
            })
            .expect("the step's live primary frame");
        frame.recorded_at_unix_ms = at_unix_ms;
        save_recording(&state, &recording)
            .unwrap_or_else(|error| panic!("step table: {}: {}", error.code, error.message));
    }

    // `record_press` of `request`, the fake Runtime answering `answer` to the input and its
    // captures taking `screens`.
    fn press<const N: usize>(
        &self,
        request: &RecordPressRequest,
        answer: Answer,
        screens: [Option<usize>; N],
    ) -> Pressed {
        let log = RefCell::new(Vec::new());
        let inputs = RefCell::new(Vec::new());
        let result = {
            let mut device = FakeDevice {
                screens: VecDeque::from(screens),
                log: &log,
            };
            let mut press = |plan: &ClickPlan| -> LabResult<CommitClickRequest> {
                log.borrow_mut().push("press".to_owned());
                inputs.borrow_mut().push(record_press_input(plan)?);
                match answer {
                    Answer::Performed => Ok(receipt("request-s11", ClickEffect::Performed)),
                    Answer::Indeterminate => Ok(receipt("request-s11", ClickEffect::Indeterminate)),
                    Answer::NotPerformed => Err(LabError::device(
                        "the fake Runtime did not perform the input",
                    )),
                }
            };
            record_press(&self.lock, request, &mut press, &mut device)
        };
        Pressed {
            result,
            log: log.into_inner(),
            inputs: inputs.into_inner(),
        }
    }

    // `record mark --step <step> --color <id>=0,0,40,40`.
    fn mark(&self, step: u32, id: &str) {
        let request = MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            step: Some(step),
            add: vec![MarkSpec::region_only(
                id.to_owned(),
                MarkFamily::Color,
                SQUARE,
            )],
            ..MarkRequest::default()
        };
        record_mark(&self.lock, &request, false, &self.ocr)
            .unwrap_or_else(|error| panic!("mark {step}: {}: {:?}", error.code, error.details));
    }

    fn recording(&self) -> Value {
        serde_json::from_slice(
            &fs::read(recording_path(&self.state(), RECORD_ID)).expect("step table"),
        )
        .expect("step table JSON")
    }

    // Every file of the recording's directory, by its `/`-separated path relative to it, with its
    // bytes, in name order.
    fn files(&self) -> Vec<(String, Vec<u8>)> {
        files_under(&lab_dir(&self.state(), RECORD_ID))
    }

    // `record stop`: the `lab` output with its status.
    fn stop(&self, dry_run: bool) -> Value {
        let outcome = record_stop(
            &self.lock,
            &RecordStopOptions {
                dry_run,
                ..RecordStopOptions::default()
            },
            &self.ocr,
        )
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

// `tap <x> <y> --record`, the frame after it 800 ms after the press.
fn tap_request(x: i32, y: i32) -> RecordPressRequest {
    RecordPressRequest {
        click: PlanClickRequest::default(),
        point_press: Some(PointPress::Tap {
            point: RecordPoint { x, y },
        }),
        after: AfterCapture::for_press(None),
    }
}

// `long-tap <x> <y> <ms> --record`.
fn long_press_request(x: i32, y: i32, duration_ms: u64) -> RecordPressRequest {
    RecordPressRequest {
        click: PlanClickRequest::default(),
        point_press: Some(PointPress::LongPress {
            point: RecordPoint { x, y },
            duration_ms,
        }),
        after: AfterCapture::for_press(None),
    }
}

// `swipe <x1> <y1> <x2> <y2> <ms> --record`.
fn swipe_request(from: (i32, i32), to: (i32, i32), duration_ms: u64) -> RecordPressRequest {
    RecordPressRequest {
        click: PlanClickRequest {
            swipe: Some(PlannedSwipe {
                from: RecordPoint {
                    x: from.0,
                    y: from.1,
                },
                to: RecordPoint { x: to.0, y: to.1 },
                duration_ms,
            }),
            ..PlanClickRequest::default()
        },
        point_press: None,
        after: AfterCapture::for_press(None),
    }
}

// The answer of a performed press, or the test's panic with its refusal.
fn performed(pressed: Pressed) -> (RecordPressOutcome, Vec<String>, Vec<InputAction>) {
    let outcome = pressed.result.unwrap_or_else(|error| {
        panic!(
            "the press: {}: {} {:?}",
            error.code, error.message, error.details
        )
    });
    (outcome, pressed.log, pressed.inputs)
}

// The frame after a press as #463 S1-2 takes it when the screen after is still at once: the wait,
// one capture, 300 ms, a capture still with it, nothing kept between.
fn after_capture(after_ms: u64) -> Value {
    json!({
        "after_ms": after_ms,
        "still_max_ms": 5000,
        "still": true,
        "waited_ms": after_ms + 300,
        "captures": 2,
        "kept": 0
    })
}

#[test]
fn s11_a_tap_on_a_new_recording_records_the_screens_before_and_after() {
    let bench = Bench::new();
    let (outcome, log, inputs) = performed(bench.press(
        &tap_request(100, 200),
        Answer::Performed,
        [Some(0), Some(1), Some(1)],
    ));
    // One capture opens step 1, the input, the wait, one capture, 300 ms (#463 S1-2), a capture
    // still with it, which opens step 2.
    assert_eq!(
        log,
        [
            "capture", "press", "wait 800", "capture", "wait 300", "capture"
        ]
    );
    assert_eq!(inputs, [InputAction::Tap { x: 100, y: 200 }]);

    let recording = bench.recording();
    let steps = recording["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 2, "{recording}");
    let step = &steps[0];
    let click = &step["click"];
    assert_eq!(click["rect"], json!(rect(100, 200, 1, 1)), "{step}");
    assert_eq!(click["source"], "point", "{step}");
    assert!(click["gesture"].is_null(), "{step}");
    let execution = &click["execution"];
    assert_eq!(execution["effect"], "performed", "{step}");
    assert_eq!(execution["point"], json!({"x": 100, "y": 200}), "{step}");
    assert_eq!(execution["point_rule"], "explicit", "{step}");
    assert!(execution["carrier_package"].is_null(), "{step}");
    assert_eq!(execution["req_id"], "request-s11", "{step}");
    assert_eq!(execution["action_id"], "request-s11-action", "{step}");
    assert_eq!(step["frames"][0]["frame_id"], "f0001", "{step}");
    assert_eq!(execution["before_frame"], "f0001", "{step}");
    assert_eq!(execution["after_frame"], "f0002", "{step}");
    assert_eq!(execution["after_capture"], after_capture(800), "{step}");
    assert_eq!(step["closed"], true, "{step}");
    assert_eq!(step["closed_by"], "click", "{step}");
    let next = &steps[1];
    assert_eq!(next["index"], 2, "{next}");
    assert_eq!(next["frames"][0]["frame_id"], "f0002", "{next}");
    assert_eq!(next["closed"], false, "{next}");
    assert!(next["click"].is_null(), "{next}");

    // The answer's `record`: the press, the frame before it and the frame after it.
    assert_eq!(outcome.click.status, "press_recorded");
    assert_eq!(outcome.click.record_id, RECORD_ID);
    assert_eq!(outcome.click.record_started_at_unix_ms, STARTED_AT);
    assert_eq!(outcome.click.step, 1);
    assert!(outcome.click.step_closed);
    assert_eq!(outcome.before.frame_id, "f0001");
    assert_eq!(outcome.before.path, step["frames"][0]["path"]);
    assert_eq!(outcome.before.sha256, step["frames"][0]["sha256"]);
    assert!(outcome.before.refreshed);
    assert!(outcome.warnings.is_empty());
    let record = serde_json::to_value(&outcome).expect("the record");
    assert_eq!(record["status"], "press_recorded", "{record}");
    assert_eq!(record["step"], 1, "{record}");
    assert_eq!(record["click"], *click, "{record}");
    assert_eq!(record["warnings"], json!([]), "{record}");
    assert_eq!(
        record["after"],
        json!({
            "step": 2,
            "frame_id": "f0002",
            "path": next["frames"][0]["path"],
            "sha256": next["frames"][0]["sha256"],
            "after_ms": 800,
            "still_max_ms": 5000,
            "still": true,
            "waited_ms": 1100,
            "captures": 2,
            "kept": 0
        }),
        "{record}"
    );
}

#[test]
fn s11_a_long_press_is_a_long_press_of_one_pixel() {
    let bench = Bench::new();
    let (outcome, log, inputs) = performed(bench.press(
        &long_press_request(640, 360, 800),
        Answer::Performed,
        [Some(0), Some(1), Some(1)],
    ));
    assert_eq!(
        log,
        [
            "capture", "press", "wait 800", "capture", "wait 300", "capture"
        ]
    );
    assert_eq!(
        inputs,
        [InputAction::LongTap {
            x: 640,
            y: 360,
            duration_ms: 800
        }]
    );
    let recording = bench.recording();
    let click = &recording["steps"][0]["click"];
    assert_eq!(click["rect"], json!(rect(640, 360, 1, 1)), "{click}");
    assert_eq!(click["source"], "point", "{click}");
    assert_eq!(
        click["gesture"],
        json!({"kind": "long_press", "duration_ms": 800}),
        "{click}"
    );
    let execution = &click["execution"];
    assert_eq!(execution["point"], json!({"x": 640, "y": 360}), "{click}");
    assert_eq!(
        execution["to_point"],
        json!({"x": 640, "y": 360}),
        "{click}"
    );
    assert_eq!(execution["duration_ms"], 800, "{click}");
    assert_eq!(execution["before_frame"], "f0001", "{click}");
    assert_eq!(execution["after_frame"], "f0002", "{click}");
    assert_eq!(outcome.click.step, 1);
    assert_eq!(outcome.after.map(|after| after.step), Some(2));
}

#[test]
fn s11_a_swipe_is_the_recorded_swipe_of_one_pixel_boxes() {
    let bench = Bench::new();
    let (outcome, log, inputs) = performed(bench.press(
        &swipe_request((900, 400), (300, 400), 300),
        Answer::Performed,
        [Some(0), Some(1), Some(1)],
    ));
    assert_eq!(
        log,
        [
            "capture", "press", "wait 800", "capture", "wait 300", "capture"
        ]
    );
    assert_eq!(
        inputs,
        [InputAction::Swipe {
            x1: 900,
            y1: 400,
            x2: 300,
            y2: 400,
            duration_ms: 300
        }]
    );
    let recording = bench.recording();
    let click = &recording["steps"][0]["click"];
    assert_eq!(click["rect"], json!(rect(900, 400, 1, 1)), "{click}");
    assert_eq!(click["source"], "swipe", "{click}");
    assert_eq!(
        click["gesture"],
        json!({"kind": "drag", "to": rect(300, 400, 1, 1), "duration_ms": 300}),
        "{click}"
    );
    let execution = &click["execution"];
    assert_eq!(execution["point"], json!({"x": 900, "y": 400}), "{click}");
    assert_eq!(
        execution["to_point"],
        json!({"x": 300, "y": 400}),
        "{click}"
    );
    assert_eq!(execution["point_rule"], "explicit", "{click}");
    assert!(execution["carrier_package"].is_null(), "{click}");
    assert_eq!(execution["before_frame"], "f0001", "{click}");
    assert_eq!(execution["after_frame"], "f0002", "{click}");
    assert_eq!(outcome.click.status, "press_recorded");
}

#[test]
fn s11_an_uncertain_answer_is_indeterminate_and_takes_no_frame_after() {
    let bench = Bench::new();
    let pressed = bench.press(&tap_request(100, 200), Answer::Indeterminate, [Some(0)]);
    // No wait and no capture after the press.
    assert_eq!(pressed.log, ["capture", "press"]);
    let error = pressed.result.expect_err("an indeterminate press");
    assert_eq!(
        error.code, "record_click_indeterminate",
        "{}",
        error.message
    );
    let details = error.details.clone().expect("details");
    assert_eq!(
        details["record"]["status"], "press_indeterminate",
        "{details}"
    );
    assert_eq!(details["record"]["step"], 1, "{details}");
    assert_eq!(details["record"]["step_closed"], false, "{details}");
    assert_eq!(
        details["record"]["before"]["frame_id"], "f0001",
        "{details}"
    );
    assert!(details["record"]["after"].is_null(), "{details}");

    let recording = bench.recording();
    let steps = recording["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 1, "{recording}");
    let execution = &steps[0]["click"]["execution"];
    assert_eq!(execution["effect"], "indeterminate", "{recording}");
    assert_eq!(execution["before_frame"], "f0001", "{recording}");
    assert!(execution["after_frame"].is_null(), "{recording}");
    assert!(execution["after_capture"].is_null(), "{recording}");
    assert_eq!(steps[0]["closed"], false, "{recording}");
}

#[test]
fn s11_a_press_not_performed_leaves_every_file_as_it_was() {
    let bench = Bench::new();
    // The open step, whose frame stands: no frame is taken before the press (#463 S1-2: its frame
    // is young).
    bench.capture(0);
    bench.recorded_at(1, UNREACHED_UNIX_MS);
    let before = bench.files();
    let pressed = bench.press(&tap_request(100, 200), Answer::NotPerformed, []);
    assert_eq!(pressed.log, ["press"]);
    let error = pressed.result.expect_err("a press not performed");
    assert_eq!(error.code, LabError::device("").code, "{}", error.message);
    assert_eq!(bench.files(), before, "a file changed");
}

#[test]
fn s11_a_refusal_before_the_press_presses_nothing() {
    let bench = Bench::new();
    bench.capture(0);
    // #463 S1-2: the open step's frame is young, so no frame is taken before the press.
    bench.recorded_at(1, UNREACHED_UNIX_MS);
    let before = bench.files();
    let too_long = RecordPressRequest {
        after: AfterCapture::for_press(Some(60_001)),
        ..tap_request(100, 200)
    };
    for (case, request, argument, reason) in [
        (
            "a point outside the frame",
            tap_request(1280, 10),
            "point",
            "outside_frame",
        ),
        (
            "a long press outside the frame",
            long_press_request(10, 720, 800),
            "point",
            "outside_frame",
        ),
        (
            "a long press of 0 ms",
            long_press_request(10, 10, 0),
            "duration_ms",
            "duration_range",
        ),
        ("a wait over 60000 ms", too_long, "after_ms", "range"),
    ] {
        let pressed = bench.press(&request, Answer::Performed, []);
        assert!(pressed.log.is_empty(), "{case}: {:?}", pressed.log);
        assert!(pressed.inputs.is_empty(), "{case}");
        let error = pressed.result.expect_err(case);
        assert_eq!(
            error.code,
            LabError::usage("").code,
            "{case}: {}",
            error.message
        );
        let details = error.details.clone().unwrap_or(Value::Null);
        assert_eq!(details["argument"], argument, "{case}: {details}");
        assert_eq!(details["reason"], reason, "{case}: {details}");
        assert_eq!(bench.files(), before, "{case}: a file changed");
    }
}

#[test]
fn s11_a_step_that_declares_a_click_presses_nothing() {
    let bench = Bench::new();
    bench.capture(0);
    // `record mark` declares the open step's click.
    let request = MarkRequest {
        schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
        add: vec![MarkSpec::region_only(
            "state/square".to_owned(),
            MarkFamily::Color,
            SQUARE,
        )],
        click: Some(ClickSpec {
            from: Some("state/square".to_owned()),
            ..ClickSpec::default()
        }),
        ..MarkRequest::default()
    };
    record_mark(&bench.lock, &request, false, &bench.ocr)
        .unwrap_or_else(|error| panic!("record mark: {}: {:?}", error.code, error.details));
    let before = bench.files();
    for request in [
        tap_request(100, 200),
        swipe_request((900, 400), (300, 400), 300),
    ] {
        let pressed = bench.press(&request, Answer::Performed, []);
        assert!(pressed.log.is_empty(), "{:?}", pressed.log);
        let error = pressed.result.expect_err("a declared click");
        assert_eq!(error.code, "record_click_exists", "{}", error.message);
        assert_eq!(bench.files(), before, "a file changed");
    }
}

#[test]
fn s11_a_frame_after_that_cannot_be_taken_leaves_the_press() {
    let bench = Bench::new();
    let pressed = bench.press(&tap_request(100, 200), Answer::Performed, [Some(0), None]);
    assert_eq!(pressed.log, ["capture", "press", "wait 800", "capture"]);
    let error = pressed.result.expect_err("the capture fails");
    assert_eq!(error.code, capture_failure().code, "{}", error.message);
    assert_eq!(error.class, capture_failure().class);
    let details = error.details.clone().expect("details");
    let leading = details
        .as_object()
        .expect("object details")
        .keys()
        .take(2)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(leading, ["record", "after"], "{details}");
    assert_eq!(
        details["after"],
        json!({"after_ms": 800, "taken": false}),
        "{details}"
    );
    assert_eq!(details["record"]["status"], "press_recorded", "{details}");
    assert_eq!(details["record"]["step"], 1, "{details}");
    assert_eq!(details["record"]["step_closed"], true, "{details}");
    assert!(details["record"]["after"].is_null(), "{details}");

    let recording = bench.recording();
    let steps = recording["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 1, "{recording}");
    assert_eq!(steps[0]["closed"], true, "{recording}");
    let execution = &steps[0]["click"]["execution"];
    assert_eq!(execution["effect"], "performed", "{recording}");
    assert_eq!(execution["before_frame"], "f0001", "{recording}");
    assert!(execution["after_frame"].is_null(), "{recording}");
}

#[test]
fn s11_marked_afterwards_the_recording_generates_a_one_pixel_click_the_kernel_admits() {
    let bench = Bench::new();
    performed(bench.press(
        &tap_request(100, 200),
        Answer::Performed,
        [Some(0), Some(1), Some(1)],
    ));
    bench.mark(1, "first_square");
    bench.mark(2, "last_square");
    // The dry run generates the package and its kernel admission takes it.
    let checked = bench.stop(true);
    assert_eq!(checked["lab_status"], "validated", "{checked}");
    let generated = bench.stop(false);
    assert_eq!(generated["lab_status"], "generated", "{generated}");
    assert_eq!(generated["click_steps"], 1, "{generated}");
    let path = generated["path"].as_str().expect("package path");
    let bytes = fs::read(path).expect("package file");
    let container = if generated["container"] == "zip" {
        ContentContainer::Zip
    } else {
        ContentContainer::Json
    };
    let entries = expand_content_container(&bytes, container, ContainmentLimits::default())
        .expect("package container");
    let task: Value = serde_json::from_slice(
        entries
            .get(&format!("resources/operations/{TASK_ID}/task.json"))
            .expect("task.json"),
    )
    .expect("task JSON");
    assert_eq!(
        task["operations"][0]["click"],
        json!({"kind": "rect", "x": 100, "y": 200, "width": 1, "height": 1}),
        "{task}"
    );
}

#[test]
fn s11_a_recording_without_such_a_press_has_no_new_member() {
    let bench = Bench::new();
    bench.capture(0);
    // `do --capture --record --tap-rect`: planned on the open step, performed by the Runtime.
    let plan = record_plan_click(
        &bench.lock,
        &PlanClickRequest {
            tap_rect: Some(rect(600, 300, 80, 40)),
            ..PlanClickRequest::default()
        },
    )
    .unwrap_or_else(|error| panic!("plan: {}: {}", error.code, error.message));
    let committed = record_commit_click(
        &bench.lock,
        &plan,
        &CommitClickRequest {
            carrier_package: opaque(json!({"package_label": "neutral.package"})),
            lease_id: opaque(json!("lease-s11")),
            ..receipt("request-do", ClickEffect::Performed)
        },
    )
    .unwrap_or_else(|error| panic!("press: {}: {}", error.code, error.message));
    assert_eq!(committed.status, "click_recorded");
    // The frame after it, as `capture --record` takes it.
    bench.capture(1);

    let recording = bench.recording();
    let text = recording.to_string();
    for member in ["before_frame", "after_frame", "after_capture"] {
        assert!(!text.contains(member), "{member}: {recording}");
    }
    let members = recording["steps"][0]["click"]["execution"]
        .as_object()
        .expect("the execution")
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let before_the_slice = [
        "point",
        "point_rule",
        "effect",
        "carrier_package",
        "req_id",
        "correlation_id",
        "action_id",
        "lease_id",
        "failure",
        "before",
        "after",
        "executed_at_unix_ms",
    ]
    .map(str::to_owned)
    .into_iter()
    .collect::<BTreeSet<_>>();
    assert_eq!(members, before_the_slice, "{recording}");
}
