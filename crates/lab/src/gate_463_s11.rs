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
//   of the recording keeps its bytes (fix round 2: on an open step with a mark, whose capture
//   before the press is not stored).
// - `s11_a_refusal_before_the_press_presses_nothing`: a point outside the frame (`validation_failed`,
//   `outside_frame`), a long press of 0 ms (`duration_range`) and a wait over 60000 ms (`range`)
//   (fix round 2: and a swipe outside the frame or of 0 ms, with the same argument and reason):
//   nothing captured, nothing pressed, nothing written.
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
// Fix round 2 (B0: every press is sent on a frame captured immediately before it, with the
// Runtime's reference of that frame):
// - `s11_two_presses_each_capture_the_frame_they_are_sent_on`: two taps in a row: capture, press,
//   wait, capture, then capture, press, wait, capture; the second capture of the open step 2
//   replaces its live primary frame (f0002 superseded, f0003), each press's `before_frame` is its
//   own capture (f0001, f0003), step 1's `after_frame` stays f0002, the frame its after capture
//   opened step 2 with, and each press is handed the reference of its own before capture.
// - `s11_after_a_failed_after_capture_the_next_capture_is_the_after_frame_once`: the after capture
//   of a tap fails; the next tap's capture opens step 2 and becomes step 1's `after_frame` (f0002),
//   step 1 gets no `after_capture`, and the frame after the second tap (f0003) leaves it as it is.
// - `s11_a_step_with_marks_is_pressed_on_a_capture_that_is_not_stored`: the open step carries a
//   mark: the capture before the tap is not stored (step 1 keeps its one frame, no file of it), the
//   press's `before_frame` is the stored f0001, `before` says `refreshed` false and
//   `sent_on_unstored_capture` true, and the press is handed that capture's reference.
// - `s11_an_observation_without_a_frame_id_is_refused_before_the_press`: a capture without a
//   frame id, on a new recording and on an open step: `device_error`, nothing pressed, every file
//   as it was.
// Fix round 3 (the press sent on a capture that was not stored is said in the recording; one retry
// when the Runtime no longer holds the frame):
// - the step with marks above: its execution's `sent_on_unstored_capture` is true; the two presses
//   in a row above, each on a stored capture: the member is absent from both executions.
// - `s11_a_press_refused_for_its_frame_is_sent_once_more_on_a_new_capture`: the first press is
//   refused as the frame being unavailable, the second performed: capture, press, capture, press,
//   wait, capture, each press handed its own capture's reference; on a new recording the first
//   capture (f0001) stays superseded and the second (f0002) is `before_frame`; on a step with
//   marks neither is stored, `before_frame` is the stored f0001 and the execution says
//   `sent_on_unstored_capture`; `before.captured_twice` in the answer.
// - `s11_a_second_refusal_is_answered_as_it_is_and_records_no_press`: the second press is refused
//   too, as the frame being unavailable or otherwise: capture, press, capture, press and nothing
//   more; the error is the fake's own, unmarked; no click recorded; both captures stay in step 1.
// Frames are synthetic 1280x720 (a square of one colour at the top left on gray); no Runtime, no
// device; the tests read no clock and never sleep.

use crate::recording::store::{lab_dir, old_record_path, recording_path};
use crate::{
    AfterCapture, AttachFrameRequest, CapturedFrame, ClickEffect, ClickPlan, ClickSpec,
    CommitClickRequest, LAB_RECORD_MARK_SCHEMA, LabOcr, MarkFamily, MarkRequest, MarkSpec,
    OpaqueJson, PlanClickRequest, PlannedSwipe, PointPress, RecordFramePorts, RecordPoint,
    RecordPressOutcome, RecordPressRequest, RecordRect, RecordStartOptions, RecordStopOptions,
    RecordingLock, record_attach_frame, record_commit_click, record_mark, record_plan_click,
    record_press, record_press_frame_unavailable, record_press_input, record_start, record_stop,
};
use actingcommand_contract::{InputAction, InputFrameReference, LabError, LabResult};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
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
    /// Fix round 3: refused before anything was sent because the frame the input names is no
    /// longer held, as the command marks that refusal.
    FrameUnavailable,
}

/// The fake Runtime's refusal of an input it did not perform.
const NOT_PERFORMED: &str = "the fake Runtime did not perform the input";
/// Fix round 3: the fake Runtime's refusal of an input whose frame it no longer holds.
const FRAME_GONE: &str = "the fake Runtime no longer holds the frame the input names";

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

/// Fix round 2: the reference of the frame the fake Runtime committed for its `number`th capture
/// of a bench (1 the first), of the synthetic frames' size.
fn reference(number: u32) -> InputFrameReference {
    InputFrameReference {
        frame_id: serde_json::from_value(json!(format!("frame_{number:032x}")))
            .expect("a frame id"),
        width: WIDTH,
        height: HEIGHT,
    }
}

/// What one capture of the fake Runtime takes.
#[derive(Clone, Copy, Debug)]
enum Shot {
    /// Screen `index`, with the reference of the frame the Runtime committed for it.
    Screen(usize),
    /// Fix round 2: screen `index` from an observation that carries no frame id.
    Frameless(usize),
    /// The capture fails.
    Fails,
}

/// The fake Runtime's capture and the caller's clock: the shots its captures take, in order, the
/// bench's count of the captures that took a frame (each one's reference is [`reference`] of its
/// number), and what it was asked and answered, in the log it shares with the press and in
/// `captured`.
struct FakeDevice<'a> {
    shots: VecDeque<Shot>,
    count: &'a Cell<u32>,
    log: &'a RefCell<Vec<String>>,
    captured: &'a RefCell<Vec<Option<InputFrameReference>>>,
}

impl RecordFramePorts for FakeDevice<'_> {
    fn wait(&mut self, duration: Duration) {
        self.log
            .borrow_mut()
            .push(format!("wait {}", duration.as_millis()));
    }

    fn capture(&mut self) -> LabResult<CapturedFrame> {
        self.log.borrow_mut().push("capture".to_owned());
        let shot = self
            .shots
            .pop_front()
            .expect("a capture the test did not script");
        let (index, framed) = match shot {
            Shot::Screen(index) => (index, true),
            Shot::Frameless(index) => (index, false),
            Shot::Fails => return Err(capture_failure()),
        };
        self.count.set(self.count.get() + 1);
        let input_frame = framed.then(|| reference(self.count.get()));
        self.captured.borrow_mut().push(input_frame);
        Ok(CapturedFrame {
            frame: captured(index),
            input_frame,
        })
    }
}

/// What one `record_press` call saw: its result, the log of the fake (`capture`, `press`,
/// `wait <ms>`), the inputs it was given and the frame reference handed with each (fix round 2),
/// and the reference of each frame its captures took, in order.
struct Pressed {
    result: LabResult<RecordPressOutcome>,
    log: Vec<String>,
    inputs: Vec<InputAction>,
    frames: Vec<InputFrameReference>,
    captured: Vec<Option<InputFrameReference>>,
}

// A state directory with an active session of `INSTANCE` (the header `record start` writes) and its
// Lab recording, before any step. The lock is held as one command holds it and dropped before the
// directory.
struct Bench {
    lock: RecordingLock,
    ocr: LabOcr,
    temp: TempDir,
    /// Fix round 2: the fake Runtime's captures of this bench that took a frame, so that each
    /// capture's frame reference is its own across presses.
    captures: Cell<u32>,
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
        Self {
            lock,
            ocr,
            temp,
            captures: Cell::new(0),
        }
    }

    fn state(&self) -> PathBuf {
        self.temp.path().join("state")
    }

    // `capture --record` of screen `index`.
    fn capture(&self, index: usize) {
        record_attach_frame(&self.lock, captured(index))
            .unwrap_or_else(|error| panic!("frame {index}: {}: {}", error.code, error.message));
    }

    // `record_press` of `request`, the fake Runtime answering `answer` to the input and its
    // captures taking `screens` (`None`: that capture fails), each with its frame reference.
    fn press<const N: usize>(
        &self,
        request: &RecordPressRequest,
        answer: Answer,
        screens: [Option<usize>; N],
    ) -> Pressed {
        self.press_shots(
            request,
            answer,
            screens.map(|screen| screen.map_or(Shot::Fails, Shot::Screen)),
        )
    }

    // `record_press` of `request`, the fake Runtime answering `answer` to the input and its
    // captures taking `shots`.
    fn press_shots<const N: usize>(
        &self,
        request: &RecordPressRequest,
        answer: Answer,
        shots: [Shot; N],
    ) -> Pressed {
        self.press_script(request, [answer], shots)
    }

    // `record_press` of `request`, the fake Runtime answering each input it is sent with the next
    // of `answers` (fix round 3: a press sent once more meets the second) and its captures taking
    // `shots`.
    fn press_script<const M: usize, const N: usize>(
        &self,
        request: &RecordPressRequest,
        answers: [Answer; M],
        shots: [Shot; N],
    ) -> Pressed {
        let log = RefCell::new(Vec::new());
        let inputs = RefCell::new(Vec::new());
        let frames = RefCell::new(Vec::new());
        let captured = RefCell::new(Vec::new());
        let mut answers = VecDeque::from(answers);
        let result = {
            let mut device = FakeDevice {
                shots: VecDeque::from(shots),
                count: &self.captures,
                log: &log,
                captured: &captured,
            };
            let mut press = |plan: &ClickPlan,
                             frame: InputFrameReference|
             -> LabResult<CommitClickRequest> {
                log.borrow_mut().push("press".to_owned());
                inputs.borrow_mut().push(record_press_input(plan)?);
                frames.borrow_mut().push(frame);
                match answers
                    .pop_front()
                    .expect("an input the test did not script")
                {
                    Answer::Performed => Ok(receipt("request-s11", ClickEffect::Performed)),
                    Answer::Indeterminate => Ok(receipt("request-s11", ClickEffect::Indeterminate)),
                    Answer::NotPerformed => Err(LabError::device(NOT_PERFORMED)),
                    Answer::FrameUnavailable => {
                        Err(record_press_frame_unavailable(LabError::device(FRAME_GONE)))
                    }
                }
            };
            record_press(&self.lock, request, &mut press, &mut device)
        };
        Pressed {
            result,
            log: log.into_inner(),
            inputs: inputs.into_inner(),
            frames: frames.into_inner(),
            captured: captured.into_inner(),
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

// The frame after a press as S1-1 takes it: the wait, one capture, no stillness test.
fn after_capture(after_ms: u64) -> Value {
    json!({
        "after_ms": after_ms,
        "still_max_ms": 0,
        "still": null,
        "waited_ms": after_ms,
        "captures": 1,
        "kept": 0
    })
}

#[test]
fn s11_a_tap_on_a_new_recording_records_the_screens_before_and_after() {
    let bench = Bench::new();
    let (outcome, log, inputs) = performed(bench.press(
        &tap_request(100, 200),
        Answer::Performed,
        [Some(0), Some(1)],
    ));
    // One capture opens step 1, the input, the wait, one capture opens step 2.
    assert_eq!(log, ["capture", "press", "wait 800", "capture"]);
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
            "still_max_ms": 0,
            "still": null,
            "waited_ms": 800,
            "captures": 1,
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
        [Some(0), Some(1)],
    ));
    assert_eq!(log, ["capture", "press", "wait 800", "capture"]);
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
        [Some(0), Some(1)],
    ));
    assert_eq!(log, ["capture", "press", "wait 800", "capture"]);
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
    // The open step, whose frame stands. Fix round 2: the press is sent on a frame captured
    // before it, which is not stored on a step that carries a mark.
    bench.capture(0);
    bench.mark(1, "first_square");
    let before = bench.files();
    let pressed = bench.press(&tap_request(100, 200), Answer::NotPerformed, [Some(1)]);
    assert_eq!(pressed.log, ["capture", "press"]);
    let error = pressed.result.expect_err("a press not performed");
    assert_eq!(error.code, LabError::device("").code, "{}", error.message);
    assert_eq!(bench.files(), before, "a file changed");
}

#[test]
fn s11_a_refusal_before_the_press_presses_nothing() {
    let bench = Bench::new();
    bench.capture(0);
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
        // Fix round 2: a swipe's refusals name the same argument and reason.
        (
            "a swipe that ends outside the frame",
            swipe_request((10, 10), (10, 720), 300),
            "point",
            "outside_frame",
        ),
        (
            "a swipe of 0 ms",
            swipe_request((10, 10), (300, 10), 0),
            "duration_ms",
            "duration_range",
        ),
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
        [Some(0), Some(1)],
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

// Fix round 2: the answer of a performed press, borrowed, or the test's panic with its refusal.
fn answered(pressed: &Pressed) -> &RecordPressOutcome {
    pressed.result.as_ref().unwrap_or_else(|error| {
        panic!(
            "the press: {}: {} {:?}",
            error.code, error.message, error.details
        )
    })
}

// Fix round 2: the names of the stored frame files of the recording (`frames/…`).
fn frame_files(bench: &Bench) -> Vec<String> {
    bench
        .files()
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| name.starts_with("frames/"))
        .collect()
}

#[test]
fn s11_two_presses_each_capture_the_frame_they_are_sent_on() {
    let bench = Bench::new();
    let first = bench.press(
        &tap_request(100, 200),
        Answer::Performed,
        [Some(0), Some(1)],
    );
    let second = bench.press(
        &tap_request(300, 400),
        Answer::Performed,
        [Some(2), Some(0)],
    );
    // The whole sequence: each press is captured for immediately before it.
    let sequence = first
        .log
        .iter()
        .chain(&second.log)
        .map(String::as_str)
        .collect::<Vec<_>>();
    assert_eq!(
        sequence,
        [
            "capture", "press", "wait 800", "capture", "capture", "press", "wait 800", "capture"
        ]
    );
    // Each press is handed the reference of its own capture before it: the first and the third.
    assert_eq!(first.captured, [Some(reference(1)), Some(reference(2))]);
    assert_eq!(second.captured, [Some(reference(3)), Some(reference(4))]);
    assert_eq!(first.frames, [reference(1)]);
    assert_eq!(second.frames, [reference(3)]);
    assert_eq!(
        second.inputs,
        [InputAction::Tap { x: 300, y: 400 }],
        "the second input"
    );

    let first_outcome = answered(&first);
    assert_eq!(first_outcome.before.frame_id, "f0001");
    assert_eq!(
        first_outcome
            .after
            .as_ref()
            .map(|after| after.frame_id.as_str()),
        Some("f0002")
    );
    let second_outcome = answered(&second);
    assert_eq!(second_outcome.click.step, 2);
    assert_eq!(second_outcome.before.frame_id, "f0003");
    assert!(second_outcome.before.refreshed);
    assert!(!second_outcome.before.sent_on_unstored_capture);
    assert_eq!(
        second_outcome
            .after
            .as_ref()
            .map(|after| (after.step, after.frame_id.as_str())),
        Some((3, "f0004"))
    );

    let recording = bench.recording();
    let steps = recording["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 3, "{recording}");
    // Step 1: its after frame is the frame its own after capture opened step 2 with, and the
    // capture before the second press does not rewrite it.
    let first_execution = &steps[0]["click"]["execution"];
    assert_eq!(first_execution["before_frame"], "f0001", "{recording}");
    assert_eq!(first_execution["after_frame"], "f0002", "{recording}");
    assert_eq!(steps[1]["frames"][0]["frame_id"], "f0002", "{recording}");
    // Step 2: the capture before the second press is its live primary frame; the frame it opened
    // with stays, superseded, as `capture --record` leaves it.
    let frames = steps[1]["frames"].as_array().expect("step 2's frames");
    assert_eq!(frames.len(), 2, "{recording}");
    assert_eq!(frames[0]["superseded"], true, "{recording}");
    assert_eq!(frames[1]["frame_id"], "f0003", "{recording}");
    assert_ne!(frames[1]["superseded"], true, "{recording}");
    let second_execution = &steps[1]["click"]["execution"];
    assert_eq!(second_execution["before_frame"], "f0003", "{recording}");
    assert_eq!(second_execution["after_frame"], "f0004", "{recording}");
    assert_eq!(steps[1]["closed"], true, "{recording}");
    assert_eq!(steps[2]["frames"][0]["frame_id"], "f0004", "{recording}");
    assert_eq!(steps[2]["closed"], false, "{recording}");
    // Fix round 3: each press was sent on a capture that was stored, so neither execution says
    // otherwise.
    for execution in [first_execution, second_execution] {
        assert!(
            execution.get("sent_on_unstored_capture").is_none(),
            "{recording}"
        );
    }
}

#[test]
fn s11_after_a_failed_after_capture_the_next_capture_is_the_after_frame_once() {
    let bench = Bench::new();
    let failed = bench.press(&tap_request(100, 200), Answer::Performed, [Some(0), None]);
    assert_eq!(
        failed.result.as_ref().expect_err("the capture fails").code,
        capture_failure().code
    );
    let second = bench.press(
        &tap_request(300, 400),
        Answer::Performed,
        [Some(1), Some(2)],
    );
    assert_eq!(second.log, ["capture", "press", "wait 800", "capture"]);
    assert_eq!(second.frames, [reference(2)]);
    let outcome = answered(&second);
    assert_eq!(outcome.click.step, 2);
    assert_eq!(outcome.before.frame_id, "f0002");
    assert!(outcome.before.refreshed);

    let recording = bench.recording();
    let steps = recording["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 3, "{recording}");
    // Step 1: the capture before the second press opened step 2 and is its after frame, once; no
    // `after_capture` was written for it, and the frame after the second press (f0003) left it.
    let first_execution = &steps[0]["click"]["execution"];
    assert_eq!(first_execution["effect"], "performed", "{recording}");
    assert_eq!(first_execution["after_frame"], "f0002", "{recording}");
    assert!(first_execution["after_capture"].is_null(), "{recording}");
    assert_eq!(steps[1]["frames"][0]["frame_id"], "f0002", "{recording}");
    let second_execution = &steps[1]["click"]["execution"];
    assert_eq!(second_execution["before_frame"], "f0002", "{recording}");
    assert_eq!(second_execution["after_frame"], "f0003", "{recording}");
    assert_eq!(
        second_execution["after_capture"],
        after_capture(800),
        "{recording}"
    );
    assert_eq!(steps[2]["frames"][0]["frame_id"], "f0003", "{recording}");
}

#[test]
fn s11_a_step_with_marks_is_pressed_on_a_capture_that_is_not_stored() {
    let bench = Bench::new();
    bench.capture(0);
    bench.mark(1, "first_square");
    let pressed = bench.press(
        &tap_request(100, 200),
        Answer::Performed,
        [Some(1), Some(2)],
    );
    assert_eq!(pressed.log, ["capture", "press", "wait 800", "capture"]);
    // The press is handed the reference of the capture before it, which was not stored.
    assert_eq!(pressed.captured[0], Some(reference(1)));
    assert_eq!(pressed.frames, [reference(1)]);
    let outcome = answered(&pressed);
    assert_eq!(outcome.before.frame_id, "f0001");
    assert!(!outcome.before.refreshed);
    assert!(outcome.before.sent_on_unstored_capture);
    let record = serde_json::to_value(outcome).expect("the record");
    assert_eq!(
        record["before"]["sent_on_unstored_capture"], true,
        "{record}"
    );
    assert_eq!(record["before"]["refreshed"], false, "{record}");

    let recording = bench.recording();
    let steps = recording["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 2, "{recording}");
    let frames = steps[0]["frames"].as_array().expect("step 1's frames");
    assert_eq!(frames.len(), 1, "{recording}");
    assert_eq!(frames[0]["frame_id"], "f0001", "{recording}");
    let execution = &steps[0]["click"]["execution"];
    assert_eq!(execution["before_frame"], "f0001", "{recording}");
    assert_eq!(execution["after_frame"], "f0002", "{recording}");
    // Fix round 3: the recording itself says the press was sent on a capture it did not store.
    assert_eq!(execution["sent_on_unstored_capture"], true, "{recording}");
    assert_eq!(steps[1]["frames"][0]["frame_id"], "f0002", "{recording}");
    // Two stored frames: screen 0 (step 1) and screen 2 (step 2); none of screen 1.
    assert_eq!(frame_files(&bench).len(), 2, "{recording}");
}

#[test]
fn s11_an_observation_without_a_frame_id_is_refused_before_the_press() {
    for open_step in [false, true] {
        let bench = Bench::new();
        if open_step {
            bench.capture(0);
        }
        let before = bench.files();
        let pressed = bench.press_shots(
            &tap_request(100, 200),
            Answer::Performed,
            [Shot::Frameless(1)],
        );
        assert_eq!(pressed.log, ["capture"], "open step {open_step}");
        assert_eq!(pressed.captured, [None], "open step {open_step}");
        assert!(pressed.inputs.is_empty(), "open step {open_step}");
        assert!(pressed.frames.is_empty(), "open step {open_step}");
        let error = pressed
            .result
            .as_ref()
            .expect_err("a capture without a frame id");
        assert_eq!(
            error.code,
            LabError::device("").code,
            "open step {open_step}: {}",
            error.message
        );
        assert_eq!(
            bench.files(),
            before,
            "open step {open_step}: a file changed"
        );
    }
}

#[test]
fn s11_a_press_refused_for_its_frame_is_sent_once_more_on_a_new_capture() {
    // A new recording: the first capture opens step 1, the second supersedes it there.
    let bench = Bench::new();
    let pressed = bench.press_script(
        &tap_request(100, 200),
        [Answer::FrameUnavailable, Answer::Performed],
        [Shot::Screen(0), Shot::Screen(1), Shot::Screen(2)],
    );
    assert_eq!(
        pressed.log,
        [
            "capture", "press", "capture", "press", "wait 800", "capture"
        ]
    );
    // Each press is handed the reference of the capture just before it.
    assert_eq!(pressed.frames, [reference(1), reference(2)]);
    assert_eq!(
        pressed.inputs,
        [
            InputAction::Tap { x: 100, y: 200 },
            InputAction::Tap { x: 100, y: 200 }
        ]
    );
    let outcome = answered(&pressed);
    assert_eq!(outcome.click.status, "press_recorded");
    assert_eq!(outcome.click.step, 1);
    assert_eq!(outcome.before.frame_id, "f0002");
    assert!(outcome.before.refreshed);
    assert!(!outcome.before.sent_on_unstored_capture);
    assert!(outcome.before.captured_twice);
    let record = serde_json::to_value(outcome).expect("the record");
    assert_eq!(record["before"]["captured_twice"], true, "{record}");
    assert_eq!(
        outcome
            .after
            .as_ref()
            .map(|after| (after.step, after.frame_id.as_str())),
        Some((2, "f0003"))
    );
    let recording = bench.recording();
    let steps = recording["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 2, "{recording}");
    let frames = steps[0]["frames"].as_array().expect("step 1's frames");
    assert_eq!(frames.len(), 2, "{recording}");
    assert_eq!(frames[0]["frame_id"], "f0001", "{recording}");
    assert_eq!(frames[0]["superseded"], true, "{recording}");
    assert_eq!(frames[1]["frame_id"], "f0002", "{recording}");
    assert_ne!(frames[1]["superseded"], true, "{recording}");
    let execution = &steps[0]["click"]["execution"];
    assert_eq!(execution["effect"], "performed", "{recording}");
    assert_eq!(execution["before_frame"], "f0002", "{recording}");
    assert_eq!(execution["after_frame"], "f0003", "{recording}");
    assert!(
        execution.get("sent_on_unstored_capture").is_none(),
        "{recording}"
    );
    assert_eq!(steps[1]["frames"][0]["frame_id"], "f0003", "{recording}");
    // Three stored frames: screens 0, 1 and 2.
    assert_eq!(frame_files(&bench).len(), 3, "{recording}");

    // A step with marks: neither capture is stored; the press is committed on the stored frame.
    let marked = Bench::new();
    marked.capture(0);
    marked.mark(1, "first_square");
    let pressed = marked.press_script(
        &tap_request(100, 200),
        [Answer::FrameUnavailable, Answer::Performed],
        [Shot::Screen(1), Shot::Screen(1), Shot::Screen(2)],
    );
    assert_eq!(
        pressed.log,
        [
            "capture", "press", "capture", "press", "wait 800", "capture"
        ]
    );
    assert_eq!(pressed.frames, [reference(1), reference(2)]);
    let outcome = answered(&pressed);
    assert_eq!(outcome.before.frame_id, "f0001");
    assert!(!outcome.before.refreshed);
    assert!(outcome.before.sent_on_unstored_capture);
    assert!(outcome.before.captured_twice);
    let recording = marked.recording();
    let steps = recording["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 2, "{recording}");
    let frames = steps[0]["frames"].as_array().expect("step 1's frames");
    assert_eq!(frames.len(), 1, "{recording}");
    assert_eq!(frames[0]["frame_id"], "f0001", "{recording}");
    let execution = &steps[0]["click"]["execution"];
    assert_eq!(execution["before_frame"], "f0001", "{recording}");
    assert_eq!(execution["after_frame"], "f0002", "{recording}");
    assert_eq!(execution["sent_on_unstored_capture"], true, "{recording}");
    // Two stored frames: screen 0 (step 1) and screen 2 (step 2); none of screen 1.
    assert_eq!(frame_files(&marked).len(), 2, "{recording}");
}

#[test]
fn s11_a_second_refusal_is_answered_as_it_is_and_records_no_press() {
    for (second, message) in [
        (Answer::FrameUnavailable, FRAME_GONE),
        (Answer::NotPerformed, NOT_PERFORMED),
    ] {
        let bench = Bench::new();
        let pressed = bench.press_script(
            &tap_request(100, 200),
            [Answer::FrameUnavailable, second],
            [Shot::Screen(0), Shot::Screen(1)],
        );
        // Once: after the second refusal no capture, no press and no wait.
        assert_eq!(
            pressed.log,
            ["capture", "press", "capture", "press"],
            "{second:?}"
        );
        assert_eq!(pressed.frames, [reference(1), reference(2)], "{second:?}");
        // The second refusal as the command answered it, without the mark.
        let error = pressed.result.as_ref().expect_err("a second refusal");
        assert_eq!(error, &LabError::device(message), "{second:?}");
        let recording = bench.recording();
        let steps = recording["steps"].as_array().expect("steps");
        assert_eq!(steps.len(), 1, "{second:?}: {recording}");
        // No press is recorded, and both captures stay in step 1, the first superseded.
        assert!(steps[0]["click"].is_null(), "{second:?}: {recording}");
        assert_eq!(steps[0]["closed"], false, "{second:?}: {recording}");
        let frames = steps[0]["frames"].as_array().expect("step 1's frames");
        assert_eq!(frames.len(), 2, "{second:?}: {recording}");
        assert_eq!(frames[0]["superseded"], true, "{second:?}: {recording}");
        assert_eq!(frames[1]["frame_id"], "f0002", "{second:?}: {recording}");
    }
}
