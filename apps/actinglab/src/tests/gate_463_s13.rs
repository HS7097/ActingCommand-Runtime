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
//   then `restart --record` on the open step it opened, which has no mark: nothing is captured
//   before the operation; the operation is that step's effect, executed and closed, its
//   `before_frame` the step's frame (the tap's screen after); the wait handed is the default 2000
//   ms, a capture opens the next step, which is the operation's `after_frame`, with
//   `after_capture`; the answer is the plain command's with `record` and `after`.
// - `s13_an_entry_launch_has_no_screen_before_or_after_it_and_starts_from_any`: `launch --record` on
//   an empty recording opens the entry step with no frame, takes no frame and asks for no device;
//   the next press's screen before opens the step after it and is no screen after the entry step;
//   marked afterwards, `record stop` generates the package whose entry is `from: any`.
// - `s13_an_application_operation_declared_offline_needs_no_mark`: `record mark --application` on
//   an open step without marks declares the operation (the rule that asked for marks is removed).
// - `s13_the_waits_handed_to_the_screen_after_are_the_flags_or_the_defaults`: `--after-ms` and
//   `--still-max-ms` as given, `AfterCapture::for_application`'s 2000 and 30000 without them, and
//   `validation_failed` (`range`) outside 0..=60000 or for a value that is no whole number.
// Frames are synthetic 1280x720 (a square of one colour at the top left on gray); no Runtime, no
// device; the tests read no clock and never sleep. The fake's captures repeat their last screen,
// and the assertions read what the operation hands the screen after, not how many captures it
// takes.

use crate::FlagArgs;
use crate::flag_args::reads;
use crate::session_management::{
    Operated, SESSION_APP_READS, application_after_capture, record_application_operation,
};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_lab::{
    AfterCapture, ApplicationExecution, ApplicationPlan, ApplicationSpec, AttachFrameRequest,
    ClickEffect, ClickPlan, CommitClickRequest, LAB_RECORD_MARK_SCHEMA, LabError, LabOcr,
    LabResult, MarkFamily, MarkRequest, MarkSpec, OpaqueJson, PlanApplicationRequest,
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
/// operation.
struct FakeDevice<'a> {
    screens: Vec<usize>,
    taken: usize,
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
        let last = self.screens.len().saturating_sub(1);
        let Some(&index) = self.screens.get(self.taken.min(last)) else {
            panic!("a capture the test did not script");
        };
        self.taken += 1;
        Ok(captured(index))
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
        };
        let mut press = |_: &ClickPlan| -> LabResult<CommitClickRequest> { Ok(performed_tap()) };
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
    // A tap: screen 0 opens step 1, screen 1 after it opens step 2, which has no mark.
    bench.tap(100, 200, [0, 1]);
    // `session app restart --record`, without its waits: the defaults are handed on.
    let after = parsed(&["--record"]).expect("the waits");
    assert_eq!(
        after,
        AfterCapture {
            after_ms: 2_000,
            still_max_ms: 30_000
        }
    );
    let (result, log) = bench.application("restart", &after, [2]);
    let data = the_answer(result);
    // Nothing is taken before the operation, which lands on step 2; after it, the wait handed
    // and a capture.
    assert_eq!(
        log.iter().take(4).map(String::as_str).collect::<Vec<_>>(),
        ["frames", "operate 2", "wait 2000", "capture"],
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
    // The screen before: the step's frame, the tap's screen after.
    let before = live_primary(step);
    assert!(before["frame_id"].is_string(), "{step}");
    assert_eq!(executed["before_frame"], before["frame_id"], "{step}");
    assert_eq!(
        executed["before_frame"], steps[0]["click"]["execution"]["after_frame"],
        "{recording}"
    );
    // The screen after: the frame that opened step 3.
    let next = &steps[2];
    assert_eq!(next["index"], 3, "{next}");
    assert_eq!(next["closed"], false, "{next}");
    assert!(next["click"].is_null(), "{next}");
    assert!(next["application"].is_null(), "{next}");
    let after_frame = live_primary(next);
    assert!(after_frame["frame_id"].is_string(), "{next}");
    assert_eq!(executed["after_frame"], after_frame["frame_id"], "{step}");
    assert_eq!(executed["after_capture"]["after_ms"], 2_000, "{step}");

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
    assert_eq!(
        AfterCapture::for_application(None),
        AfterCapture {
            after_ms: 2_000,
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
            still_max_ms: 0
        }
    );
    assert_eq!(
        parsed(&["--still-max-ms", "60000", "--record"]).expect("the bound only"),
        AfterCapture {
            after_ms: 2_000,
            still_max_ms: 60_000
        }
    );
    for (words, argument) in [
        (["--record", "--after-ms", "60001"], "after_ms"),
        (["--record", "--still-max-ms", "60001"], "still_max_ms"),
        (["--record", "--after-ms", "soon"], "after_ms"),
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
