// SPDX-License-Identifier: AGPL-3.0-only

// #463 S1-3 (contracts/lab-recording.md, "Application steps"; register §1.404): an application
// operation in the middle of a business is recorded as the open step's effect, with the screen
// before it and the screen after it, as a press is. `record_application_operation` is what `session
// app <verb> --record` does under the recording lock: the screen before (`record_before_frame`),
// the plan, the operation sent once through the caller (a fake Runtime here), its completed receipt
// recorded, then the screen after it (`record_after_frame`, with the waits the command line gives),
// on the fake's clock and capture (`RecordFramePorts`). The presses are `tap --record`'s
// (`record_press`).
// - `s13_a_restart_in_the_middle_of_a_business_is_the_open_steps_effect_with_both_screens`: a tap,
//   then `restart --record` on the open step it opened, which has no mark: round 2, a capture
//   immediately before the operation is that step's live primary frame (the tap's screen after
//   superseded, and still the tap's `after_frame`); the operation is that step's effect, executed
//   and closed, its `before_frame` that capture; the wait handed is the default 2000 ms, a capture
//   opens the next step, which is the operation's `after_frame`, with `after_capture`; the answer
//   is the plain command's with `record` and `after`.
// - `s13_an_entry_launch_has_no_screen_before_or_after_it_and_starts_from_any`: `launch --record` on
//   an empty recording opens the entry step with no frame, takes no frame and asks for no device;
//   the next press's screen before opens the step after it and is no screen after the entry step;
//   marked afterwards, `record stop` generates the package whose entry is `from: any`.
// - `s13_an_application_operation_declared_offline_needs_no_mark`: `record mark --application` on
//   an open step without marks declares the operation (the rule that asked for marks is removed).
// - `s13_the_waits_handed_to_the_screen_after_are_the_flags_or_the_defaults`: `--after-ms` and
//   `--still-max-ms` as given, `AfterCapture::for_application`'s 2000 and 30000 without them, and
//   `validation_failed` (`range`) outside 0..=60000 or for a value that is no whole number.
// Round 2 (the parent's fix round: every press is sent on a frame captured immediately before it;
// an application operation's screen before follows the same rule):
// - `s13_an_operation_on_a_marked_step_keeps_the_stored_frame`: on an open step with a mark the
//   capture before the operation is not stored; the operation's `before_frame` is the stored frame
//   the mark was made on, and the screen after takes the next frame id.
// - `s13_two_operations_in_a_row_each_capture_the_screen_before_them`: `restart --record`, then
//   `stop --record` on the step the restart's screen after opened: each captures before it, each
//   `before_frame` is that capture, and the restart's `after_frame` is not rewritten.
// - `s13_a_press_after_an_operation_replaces_the_frame_its_screen_after_opened_the_step_with`: a tap
//   after `restart --record`: its capture before replaces the frame the restart's screen after
//   opened the step with, and the restart's `after_frame` is not rewritten.
// The train of stage one (S1-1, S1-2 and S1-3 joined): the screen after an operation, and after a
// tap, is captured until it is still (#463 S1-2), on the fake's clock, which moves only by its
// waits. No test switches the loop off. What each now captures, the fake's captures repeating
// their last screen: a tap of `Bench::tap` (every 100 ms, still after 300 ms) one capture before
// it and four after it; an operation with `AfterCapture::for_application` one capture before it
// (none on the entry step) and three after it (after 2000 ms, then every 1000 ms, still after
// 2000 ms). So:
// - the restart in the middle of a business: the tap's after loop is f0002 to f0005 (its
//   `after_frame` f0005), the capture before the restart f0006 (`before_frame`), its after loop
//   f0007 to f0009 in step 3 (`after_frame` f0009, `after_capture {2000, 1000, 2000, 30000, true,
//   4000, 3, 2}`), the answer's `record.warnings` empty;
// - the entry launch: no capture for it; the tap after it, one before (f0001) and four after;
// - the operation declared offline: no capture;
// - the waits: the four flags of a press, the application's own interval and stillness by
//   default, and a bound of more than 120 intervals refused before anything is sent;
// - the operation on a marked step: one capture before it (not stored), three after it (f0002 to
//   f0004, the last the `after_frame`);
// - two operations in a row: a tap (one and four), then each operation one before it and three
//   after it;
// - a press after an operation: a tap (one and four), the restart (one and three), the tap (one
//   and four).
// Frames are synthetic 1280x720 (a square of one colour at the top left on gray); no Runtime, no
// device; the tests read no clock and never sleep.

use crate::FlagArgs;
use crate::flag_args::reads;
use crate::session_management::{
    Operated, SESSION_APP_READS, application_after_capture, record_application_operation,
};
use actingcommand_contract::InputFrameReference;
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_lab::{
    AfterCapture, ApplicationExecution, ApplicationPlan, ApplicationSpec, AttachFrameRequest,
    CapturedFrame, ClickEffect, ClickPlan, CommitClickRequest, LAB_RECORD_MARK_SCHEMA, LabError,
    LabOcr, LabResult, MarkFamily, MarkRequest, MarkSpec, OpaqueJson, PlanApplicationRequest,
    PlanClickRequest, PointPress, RecordFramePorts, RecordPoint, RecordPressRequest, RecordRect,
    RecordStartOptions, RecordStopOptions, RecordingLock, record_attach_frame, record_mark,
    record_press, record_start, record_stop,
};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0463_s13";
const TASK_ID: &str = "neutral_task";
const STARTED_AT: u64 = 1_700_000_000_000;
const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
const GRAY: [u8; 3] = [60, 60, 60];
/// The colour of each screen's square.
const COLORS: [[u8; 3]; 3] = [[230, 20, 20], [20, 230, 20], [20, 20, 230]];
/// Each screen's square, a colour mark's region.
const SQUARE: RecordRect = RecordRect {
    x: 0,
    y: 0,
    width: 40,
    height: 40,
};

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

fn opaque(value: Value) -> Option<OpaqueJson> {
    Some(OpaqueJson::from_serializable(&value).expect("opaque value"))
}

// The input receipt of a performed tap, as `tap --record` hands it to the recording.
fn performed_tap() -> CommitClickRequest {
    CommitClickRequest {
        effect: ClickEffect::Performed,
        has_failure: false,
        carrier_package: None,
        req_id: opaque(json!("request-tap")),
        correlation_id: opaque(json!("request-tap-correlation")),
        action_id: opaque(json!("request-tap-action")),
        lease_id: None,
        failure: None,
        before: None,
        after: None,
    }
}

// The completed receipt of the application operation, as `session app` hands it to the
// recording: the screens are the recording's to name.
fn completed() -> ApplicationExecution {
    ApplicationExecution {
        request_id: "request-s13".to_owned(),
        correlation_id: "request-s13-correlation".to_owned(),
        action_id: "request-s13-action".to_owned(),
        receipt_state: "completed".to_owned(),
        application_event_ids: vec!["event-s13-intent".to_owned(), "event-s13-done".to_owned()],
        before_frame: None,
        after_frame: None,
        after_capture: None,
    }
}

/// The fake Runtime's capture and the caller's clock: the screens its captures take, in order, the
/// last one again once they run out, and what it was asked, in the log it shares with the
/// operation. The train of stage one: the clock (`now`) moves only by the waits, so that the after
/// loop of #463 S1-2 ends when the repeated screen has stood for `still_ms`; a capture takes no
/// time.
struct FakeDevice<'a> {
    screens: Vec<usize>,
    taken: usize,
    log: &'a RefCell<Vec<String>>,
    now: Duration,
}

impl RecordFramePorts for FakeDevice<'_> {
    fn wait(&mut self, duration: Duration) {
        self.now += duration;
        self.log
            .borrow_mut()
            .push(format!("wait {}", duration.as_millis()));
    }

    fn elapsed(&mut self) -> Duration {
        self.now
    }

    fn capture(&mut self) -> LabResult<CapturedFrame> {
        self.log.borrow_mut().push("capture".to_owned());
        let last = self.screens.len().saturating_sub(1);
        let Some(&index) = self.screens.get(self.taken.min(last)) else {
            panic!("a capture the test did not script");
        };
        self.taken += 1;
        Ok(CapturedFrame {
            frame: captured(index),
            input_frame: Some(reference(self.taken)),
        })
    }
}

/// Round 2: the reference of the frame the fake Runtime committed for its `number`th capture of one
/// command (1 the first), of the synthetic frames' size; an application operation names none.
fn reference(number: usize) -> InputFrameReference {
    InputFrameReference {
        frame_id: serde_json::from_value(json!(format!("frame_{number:032x}")))
            .expect("a frame id"),
        width: WIDTH,
        height: HEIGHT,
    }
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
            state.join(format!("record-{INSTANCE}.json")),
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
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_463_s13", "Lab-CLI")
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

    // `tap <x> <y> --record`, the fake Runtime performing it and its captures taking `screens`.
    fn tap<const N: usize>(&self, x: i32, y: i32, screens: [usize; N]) {
        let log = RefCell::new(Vec::new());
        let mut device = FakeDevice {
            screens: screens.to_vec(),
            taken: 0,
            log: &log,
            now: Duration::ZERO,
        };
        let mut press = |_: &ClickPlan, _: InputFrameReference| -> LabResult<CommitClickRequest> {
            Ok(performed_tap())
        };
        let request = RecordPressRequest {
            click: PlanClickRequest::default(),
            point_press: Some(PointPress::Tap {
                point: RecordPoint { x, y },
            }),
            after: AfterCapture::for_press(None),
        };
        record_press(&self.lock, &request, &mut press, &mut device).unwrap_or_else(|error| {
            panic!("tap: {}: {} {:?}", error.code, error.message, error.details)
        });
    }

    // `session app <verb> --record` with the waits `after`, the fake Runtime completing the
    // operation and its captures taking `screens`: the answer, and the log of the device being
    // asked for (`frames`), the operation (`operate <step>`), the waits and the captures.
    fn application<const N: usize>(
        &self,
        verb: &str,
        after: &AfterCapture,
        screens: [usize; N],
    ) -> (LabResult<Value>, Vec<String>) {
        let log = RefCell::new(Vec::new());
        let result = record_application_operation(
            &self.lock,
            &PlanApplicationRequest {
                verb: verb.to_owned(),
            },
            after,
            || {
                log.borrow_mut().push("frames".to_owned());
                Ok(FakeDevice {
                    screens: screens.to_vec(),
                    taken: 0,
                    log: &log,
                    now: Duration::ZERO,
                })
            },
            |plan: &ApplicationPlan| -> LabResult<Operated> {
                log.borrow_mut().push(format!("operate {}", plan.step));
                Ok(Operated {
                    execution: completed(),
                    data: json!({"status": "completed", "action": verb}),
                })
            },
        );
        (result, log.into_inner())
    }

    // `record mark --step <step> --color <id>=0,0,40,40`, with `--page <page>` when given.
    fn mark(&self, step: u32, id: &str, page: Option<&str>) {
        let request = MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            step: Some(step),
            page: page.map(str::to_owned),
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
        let path = self
            .state()
            .join("record-artifacts")
            .join(RECORD_ID)
            .join("lab")
            .join("recording.json");
        serde_json::from_slice(&fs::read(path).expect("step table")).expect("step table JSON")
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

// The step's live (not superseded) primary frame.
fn live_primary(step: &Value) -> Value {
    step["frames"]
        .as_array()
        .expect("the step's frames")
        .iter()
        .rev()
        .find(|frame| frame["role"] == "primary" && frame["superseded"] == false)
        .cloned()
        .unwrap_or(Value::Null)
}

// What `session app <verb>` takes for the screen after a recorded operation from `words`.
fn parsed(words: &[&str]) -> LabResult<AfterCapture> {
    let words = words
        .iter()
        .map(|word| (*word).to_owned())
        .collect::<Vec<_>>();
    let after = FlagArgs::parse_reading(&words, &SESSION_APP_READS)
        .and_then(|flags| application_after_capture(&flags));
    reads::take();
    after
}

fn the_answer(result: LabResult<Value>) -> Value {
    result.unwrap_or_else(|error| {
        panic!(
            "the operation: {}: {} {:?}",
            error.code, error.message, error.details
        )
    })
}

// The members a completed receipt an earlier build recorded never has.
const SCREEN_MEMBERS: [&str; 3] = ["before_frame", "after_frame", "after_capture"];

#[test]
fn s13_a_restart_in_the_middle_of_a_business_is_the_open_steps_effect_with_both_screens() {
    let bench = Bench::new();
    // A tap: screen 0 opens step 1, screen 1 after it opens step 2, which has no mark. The train
    // of stage one: the tap's after loop (#463 S1-2, every 100 ms, still after 300 ms) takes screen
    // 1 four times, f0002 to f0005, the last its `after_frame`.
    bench.tap(100, 200, [0, 1]);
    // `session app restart --record`, without its waits: the defaults are handed on.
    let after = parsed(&["--record"]).expect("the waits");
    assert_eq!(
        after,
        AfterCapture {
            after_ms: 2_000,
            interval_ms: Some(1_000),
            still_ms: Some(2_000),
            still_max_ms: 30_000
        }
    );
    let (result, log) = bench.application("restart", &after, [2, 0]);
    let data = the_answer(result);
    // Round 2: a capture immediately before the operation, which lands on step 2; after it, the
    // wait handed and a capture. The train of stage one: then one capture every 1000 ms until
    // screen 0 has stood for 2000 ms: three captures after the operation.
    assert_eq!(
        log,
        [
            "frames",
            "capture",
            "operate 2",
            "wait 2000",
            "capture",
            "wait 1000",
            "capture",
            "wait 1000",
            "capture"
        ],
        "{log:?}"
    );

    let recording = bench.recording();
    let steps = recording["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 3, "{recording}");
    let step = &steps[1];
    assert_eq!(step["index"], 2, "{step}");
    assert_eq!(step["marks"], json!([]), "{step}");
    assert!(step["click"].is_null(), "{step}");
    assert_eq!(step["closed"], true, "{step}");
    assert_eq!(step["closed_by"], "application", "{step}");
    let application = &step["application"];
    assert_eq!(application["action"], "restart", "{step}");
    assert_eq!(application["cli_verb"], "restart", "{step}");
    assert_eq!(application["source"], "executed", "{step}");
    assert_eq!(application["attempts"], json!([]), "{step}");
    let executed = &application["executed"];
    assert_eq!(executed["request_id"], "request-s13", "{step}");
    // The screen before: the capture before the operation, the step's live primary frame; the
    // frames the tap's screen after took stay in it, superseded, and the last of them (the train of
    // stage one: f0005, the tap's after loop's last) is still the tap's `after_frame`.
    let before = live_primary(step);
    assert_eq!(before["frame_id"], "f0006", "{step}");
    assert_eq!(executed["before_frame"], "f0006", "{step}");
    let frames = step["frames"].as_array().expect("step 2's frames");
    assert_eq!(
        frames
            .iter()
            .map(|frame| (frame["frame_id"].clone(), frame["superseded"].clone()))
            .collect::<Vec<_>>(),
        [
            (json!("f0002"), json!(true)),
            (json!("f0003"), json!(true)),
            (json!("f0004"), json!(true)),
            (json!("f0005"), json!(true)),
            (json!("f0006"), json!(false))
        ],
        "{step}"
    );
    assert_eq!(
        steps[0]["click"]["execution"]["after_frame"], "f0005",
        "{recording}"
    );
    // The screen after: the train of stage one: the three captures after the operation are the
    // frames of step 3, which the first opened (f0007, f0008, f0009), the last its live primary
    // frame and the operation's `after_frame`.
    let next = &steps[2];
    assert_eq!(next["index"], 3, "{next}");
    assert_eq!(next["closed"], false, "{next}");
    assert!(next["click"].is_null(), "{next}");
    assert!(next["application"].is_null(), "{next}");
    assert_eq!(next["frames"].as_array().map(Vec::len), Some(3), "{next}");
    let after_frame = live_primary(next);
    assert_eq!(after_frame["frame_id"], "f0009", "{next}");
    assert_eq!(executed["after_frame"], after_frame["frame_id"], "{step}");
    assert_eq!(
        executed["after_capture"],
        json!({
            "after_ms": 2_000,
            "interval_ms": 1_000,
            "still_ms": 2_000,
            "still_max_ms": 30_000,
            "still": true,
            "waited_ms": 4_000,
            "captures": 3,
            "kept": 2
        }),
        "{step}"
    );

    // The answer: the plain command's, with the operation as recorded and the screen after it.
    assert_eq!(data["status"], "completed", "{data}");
    assert_eq!(data["action"], "restart", "{data}");
    let record = &data["record"];
    assert_eq!(record["status"], "application_recorded", "{record}");
    assert_eq!(record["record_id"], RECORD_ID, "{record}");
    assert_eq!(record["record_started_at_unix_ms"], STARTED_AT, "{record}");
    assert_eq!(record["step"], 2, "{record}");
    assert_eq!(record["step_opened"], false, "{record}");
    assert_eq!(record["step_closed"], true, "{record}");
    assert_eq!(record["application"], *application, "{record}");
    assert_eq!(record["after"]["step"], 3, "{record}");
    assert_eq!(
        record["after"]["frame_id"], after_frame["frame_id"],
        "{record}"
    );
    assert_eq!(record["after"]["path"], after_frame["path"], "{record}");
    assert_eq!(record["after"]["sha256"], after_frame["sha256"], "{record}");
    assert_eq!(record["after"]["after_ms"], 2_000, "{record}");
    // The train of stage one: the screen was found still, so no warning, in the member a press's
    // answer carries `after_not_still` in.
    assert_eq!(record["after"]["still"], true, "{record}");
    assert_eq!(record["warnings"], json!([]), "{record}");
}

#[test]
fn s13_an_entry_launch_has_no_screen_before_or_after_it_and_starts_from_any() {
    let bench = Bench::new();
    let (result, log) = bench.application("launch", &AfterCapture::for_application(None), []);
    let data = the_answer(result);
    // No device is asked for: nothing is captured or waited for.
    assert_eq!(log, ["operate 1"]);
    let recording = bench.recording();
    let steps = recording["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 1, "{recording}");
    let entry = &steps[0];
    assert_eq!(entry["index"], 1, "{entry}");
    assert_eq!(entry["frames"], json!([]), "{entry}");
    assert_eq!(entry["closed_by"], "application", "{entry}");
    assert_eq!(entry["application"]["action"], "launch", "{entry}");
    let executed = entry["application"]["executed"]
        .as_object()
        .expect("the completed receipt");
    for member in SCREEN_MEMBERS {
        assert!(!executed.contains_key(member), "{member}: {entry}");
    }
    assert_eq!(data["record"]["step"], 1, "{data}");
    assert_eq!(data["record"]["step_opened"], true, "{data}");
    assert!(data["record"]["after"].is_null(), "{data}");

    // The first press: its screen before opens the step after the entry step, and is no screen
    // after the entry step.
    bench.tap(100, 200, [0, 1]);
    let pressed = bench.recording();
    let pressed_steps = pressed["steps"].as_array().expect("steps");
    assert_eq!(pressed_steps.len(), 3, "{pressed}");
    let entry_receipt = pressed_steps[0]["application"]["executed"]
        .as_object()
        .expect("the completed receipt");
    for member in SCREEN_MEMBERS {
        assert!(!entry_receipt.contains_key(member), "{member}: {pressed}");
    }
    assert_eq!(
        pressed_steps[1]["frames"][0]["frame_id"], "f0001",
        "{pressed}"
    );
    assert_eq!(
        pressed_steps[1]["click"]["execution"]["before_frame"], "f0001",
        "{pressed}"
    );

    // Marked afterwards, the package starts from any page with the launch.
    bench.mark(2, "first_square", None);
    bench.mark(3, "last_square", Some("home"));
    let checked = bench.stop(true);
    assert_eq!(checked["lab_status"], "validated", "{checked}");
    let generated = bench.stop(false);
    assert_eq!(generated["lab_status"], "generated", "{generated}");
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
    assert_eq!(task["entry_page"], "any", "{task}");
    assert_eq!(task["operations"][0]["from"], "any", "{task}");
    assert_eq!(
        task["operations"][0]["application"],
        json!({"action": "launch"}),
        "{task}"
    );
}

#[test]
fn s13_an_application_operation_declared_offline_needs_no_mark() {
    let bench = Bench::new();
    // `capture --record`: step 1, open, without marks.
    record_attach_frame(&bench.lock, captured(0))
        .unwrap_or_else(|error| panic!("frame: {}: {}", error.code, error.message));
    let request = MarkRequest {
        schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
        application: Some(ApplicationSpec {
            action: "restart".to_owned(),
        }),
        ..MarkRequest::default()
    };
    record_mark(&bench.lock, &request, false, &bench.ocr).unwrap_or_else(|error| {
        panic!(
            "record mark --application: {}: {} {:?}",
            error.code, error.message, error.details
        )
    });
    let recording = bench.recording();
    let step = &recording["steps"][0];
    assert_eq!(step["marks"], json!([]), "{step}");
    assert_eq!(step["application"]["action"], "restart", "{step}");
    assert_eq!(step["application"]["source"], "declared", "{step}");
    assert!(step["application"]["executed"].is_null(), "{step}");
    assert_eq!(step["closed"], false, "{step}");
}

#[test]
fn s13_the_waits_handed_to_the_screen_after_are_the_flags_or_the_defaults() {
    // The train of stage one: the application's own interval and stillness (#463 S1-2 fix round
    // 3: after 2000 ms, every 1000 ms, still after 2000 ms unchanged, at most 30000 ms).
    assert_eq!(
        AfterCapture::for_application(None),
        AfterCapture {
            after_ms: 2_000,
            interval_ms: Some(1_000),
            still_ms: Some(2_000),
            still_max_ms: 30_000
        }
    );
    assert_eq!(
        parsed(&["--record"]).expect("no waits"),
        AfterCapture::for_application(None)
    );
    assert_eq!(
        parsed(&["--record", "--after-ms", "500", "--still-max-ms", "0"]).expect("both waits"),
        AfterCapture {
            after_ms: 500,
            interval_ms: Some(1_000),
            still_ms: Some(2_000),
            still_max_ms: 0
        }
    );
    assert_eq!(
        parsed(&["--still-max-ms", "60000", "--record"]).expect("the bound only"),
        AfterCapture {
            after_ms: 2_000,
            interval_ms: Some(1_000),
            still_ms: Some(2_000),
            still_max_ms: 60_000
        }
    );
    // The train of stage one: the interval and the stillness as a press takes them.
    assert_eq!(
        parsed(&["--record", "--interval-ms", "500", "--still-ms", "0"]).expect("the loop's"),
        AfterCapture {
            after_ms: 2_000,
            interval_ms: Some(500),
            still_ms: Some(0),
            still_max_ms: 30_000
        }
    );
    for (words, argument) in [
        (["--record", "--after-ms", "60001"], "after_ms"),
        (["--record", "--still-max-ms", "60001"], "still_max_ms"),
        (["--record", "--after-ms", "soon"], "after_ms"),
        // The train of stage one: 30000 ms at 100 ms is more than 120 intervals, refused before
        // the operation is sent, as a press's bound is.
        (["--record", "--interval-ms", "100"], "still_max_ms"),
    ] {
        let error = parsed(&words).expect_err("a wait outside its range");
        assert_eq!(
            error.code,
            LabError::usage("").code,
            "{words:?}: {}",
            error.message
        );
        let details = error.details.clone().unwrap_or(Value::Null);
        assert_eq!(details["argument"], argument, "{words:?}: {details}");
        assert_eq!(details["reason"], "range", "{words:?}: {details}");
    }
}

// The step's frame `frame_id`, or null.
fn frame_of(step: &Value, frame_id: &Value) -> Value {
    step["frames"]
        .as_array()
        .expect("the step's frames")
        .iter()
        .find(|frame| frame["frame_id"] == *frame_id)
        .cloned()
        .unwrap_or(Value::Null)
}

#[test]
fn s13_an_operation_on_a_marked_step_keeps_the_stored_frame() {
    let bench = Bench::new();
    // `capture --record` of screen 0: step 1, open, then marked on that frame.
    record_attach_frame(&bench.lock, captured(0))
        .unwrap_or_else(|error| panic!("frame: {}: {}", error.code, error.message));
    bench.mark(1, "first_square", None);
    let (result, log) = bench.application("restart", &AfterCapture::for_application(None), [1, 2]);
    let data = the_answer(result);
    // The capture before the operation is taken all the same, immediately before it. The train of
    // stage one: after it, three captures of screen 2 (still after 2000 ms).
    assert_eq!(
        log,
        [
            "frames",
            "capture",
            "operate 1",
            "wait 2000",
            "capture",
            "wait 1000",
            "capture",
            "wait 1000",
            "capture"
        ],
        "{log:?}"
    );

    let recording = bench.recording();
    let steps = recording["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 2, "{recording}");
    let step = &steps[0];
    // It is not stored: the step keeps the one frame its mark was made on.
    assert_eq!(step["frames"].as_array().map(Vec::len), Some(1), "{step}");
    assert_eq!(step["frames"][0]["frame_id"], "f0001", "{step}");
    assert_eq!(step["frames"][0]["superseded"], false, "{step}");
    assert_eq!(step["marks"].as_array().map(Vec::len), Some(1), "{step}");
    assert_eq!(step["closed_by"], "application", "{step}");
    let executed = &step["application"]["executed"];
    assert_eq!(executed["before_frame"], "f0001", "{step}");
    // The screen after takes the next frame id: the capture before took none. The train of stage
    // one: its three captures are step 2's frames, f0002 to f0004, the last the `after_frame`.
    let next = &steps[1];
    assert_eq!(next["frames"][0]["frame_id"], "f0002", "{next}");
    assert_eq!(live_primary(next)["frame_id"], "f0004", "{next}");
    assert_eq!(
        executed["after_frame"],
        live_primary(next)["frame_id"],
        "{recording}"
    );
    assert_eq!(executed["after_capture"]["after_ms"], 2_000, "{step}");
    assert_eq!(
        data["record"]["after"]["frame_id"], executed["after_frame"],
        "{data}"
    );
}

#[test]
fn s13_two_operations_in_a_row_each_capture_the_screen_before_them() {
    let bench = Bench::new();
    // A tap: screen 0 opens step 1, screen 1 after it opens step 2.
    bench.tap(100, 200, [0, 1]);
    let after = AfterCapture::for_application(None);
    // `restart --record` on step 2, then `stop --record` on step 3, which the restart's screen
    // after opened; neither step has a mark.
    let (first, first_log) = bench.application("restart", &after, [2, 0]);
    let restarted = the_answer(first);
    let (second, second_log) = bench.application("stop", &after, [1, 2]);
    let stopped = the_answer(second);
    // The train of stage one: after each operation, three captures (still after 2000 ms).
    let after_each = [
        "wait 2000",
        "capture",
        "wait 1000",
        "capture",
        "wait 1000",
        "capture",
    ];
    assert_eq!(
        first_log.iter().map(String::as_str).collect::<Vec<_>>(),
        [&["frames", "capture", "operate 2"][..], &after_each[..]].concat(),
        "{first_log:?}"
    );
    assert_eq!(
        second_log.iter().map(String::as_str).collect::<Vec<_>>(),
        [&["frames", "capture", "operate 3"][..], &after_each[..]].concat(),
        "{second_log:?}"
    );
    let restart_after = restarted["record"]["after"]["frame_id"].clone();
    assert!(restart_after.is_string(), "{restarted}");

    let recording = bench.recording();
    let steps = recording["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 4, "{recording}");
    let restart = &steps[1];
    assert_eq!(restart["application"]["action"], "restart", "{restart}");
    assert_eq!(
        restart["application"]["executed"]["before_frame"],
        live_primary(restart)["frame_id"],
        "{restart}"
    );
    // Its screen after is not rewritten by the capture before the next operation.
    assert_eq!(
        restart["application"]["executed"]["after_frame"], restart_after,
        "{recording}"
    );
    let stop = &steps[2];
    assert_eq!(stop["index"], 3, "{stop}");
    assert_eq!(stop["application"]["action"], "stop", "{stop}");
    assert_eq!(stop["closed_by"], "application", "{stop}");
    // The frame the restart's screen after opened step 3 with is superseded by the capture before
    // the stop, which is the stop's `before_frame`.
    assert_eq!(frame_of(stop, &restart_after)["superseded"], true, "{stop}");
    let before = live_primary(stop);
    assert!(before["frame_id"].is_string(), "{stop}");
    assert_ne!(before["frame_id"], restart_after, "{stop}");
    assert_eq!(
        stop["application"]["executed"]["before_frame"], before["frame_id"],
        "{stop}"
    );
    // The stop's screen after opened step 4.
    let last = &steps[3];
    assert_eq!(last["closed"], false, "{last}");
    assert_eq!(
        stop["application"]["executed"]["after_frame"],
        live_primary(last)["frame_id"],
        "{recording}"
    );
    assert_eq!(stopped["record"]["step"], 3, "{stopped}");
    assert_eq!(stopped["record"]["after"]["step"], 4, "{stopped}");
}

#[test]
fn s13_a_press_after_an_operation_replaces_the_frame_its_screen_after_opened_the_step_with() {
    let bench = Bench::new();
    bench.tap(100, 200, [0, 1]);
    let (result, _) = bench.application("restart", &AfterCapture::for_application(None), [2, 0]);
    let restarted = the_answer(result);
    let restart_after = restarted["record"]["after"]["frame_id"].clone();
    assert!(restart_after.is_string(), "{restarted}");
    // A tap on step 3, which the restart's screen after opened and which has no mark.
    bench.tap(300, 400, [1, 2]);

    let recording = bench.recording();
    let steps = recording["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 4, "{recording}");
    // The restart's screen after is not rewritten.
    assert_eq!(
        steps[1]["application"]["executed"]["after_frame"], restart_after,
        "{recording}"
    );
    let pressed = &steps[2];
    assert_eq!(pressed["index"], 3, "{pressed}");
    assert_eq!(pressed["closed_by"], "click", "{pressed}");
    // The press's capture before it supersedes the frame the restart's screen after opened the
    // step with, and is the press's `before_frame`.
    assert_eq!(
        frame_of(pressed, &restart_after)["superseded"],
        true,
        "{pressed}"
    );
    let before = live_primary(pressed);
    assert!(before["frame_id"].is_string(), "{pressed}");
    assert_ne!(before["frame_id"], restart_after, "{pressed}");
    assert_eq!(
        pressed["click"]["execution"]["before_frame"], before["frame_id"],
        "{pressed}"
    );
    assert_eq!(
        pressed["click"]["execution"]["after_frame"],
        live_primary(&steps[3])["frame_id"],
        "{recording}"
    );
}
