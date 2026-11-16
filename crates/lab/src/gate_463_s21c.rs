// SPDX-License-Identifier: AGPL-3.0-only

// #463 S2-1c (contracts/lab-recording.md, "Self-checks" and "What a recording still lacks: `record
// ready`"; register §1.443): `record ready` lists every failure of each self-check, not only the
// first, and says for each self-check that ran whether its list is complete. A recording made as
// stage one makes it (three taps through `record_press` with a fake Runtime, its captures taking the
// scripted screens and the caller's clock moving only by its waits; four steps whose live frames are
// f0001, f0003, f0005 and f0006), each step marked with a colour mark on its square; a mark is then
// broken in the step table, as a stage-two write that went wrong would leave it.
// - `s21c_three_frames_that_fail_the_cross_check_are_three_items_and_stop_answers_the_first`: the
//   marks of steps 2, 3 and 4 expect another colour: three `cross_check` items, by step, each with
//   its frame and page, the cross-check complete and the final package not run; `record stop`
//   answers the first, `record_step_self_mismatch` for step 2 on f0003, as before.
// - `s21c_two_self_checks_that_fail_are_listed_in_the_order_the_checks_run`: step 1's mark expects
//   another colour: the first decision's item, then the cross-check's; `record stop` answers the
//   first decision's.
// - `s21c_a_check_that_must_stop_says_so_and_lists_one`: step 2's mark declares a negative colour
//   distance, which the kernel's admission refuses: one `admission` item, `complete: false` with
//   `whole_package`, and no later check run.
// - `s21c_a_recording_that_passes_has_every_check_complete_and_is_ready`.
// Frames are synthetic 160x90 screens (a square of one colour at the top left on gray); no Runtime,
// no device; the tests read no clock and never sleep.

use crate::recording::store::{old_record_path, recording_path};
use crate::{
    AfterCapture, AttachFrameRequest, CapturedFrame, ClickEffect, ClickPlan, CommitClickRequest,
    LAB_RECORD_MARK_SCHEMA, LabOcr, MarkFamily, MarkRequest, MarkSpec, PlanClickRequest,
    PointPress, RecordFramePorts, RecordPoint, RecordPressRequest, RecordRect, RecordStartOptions,
    RecordStopOptions, RecordingLock, record_mark, record_press, record_ready, record_start,
    record_stop,
};
use actingcommand_contract::{InputFrameReference, LabError, LabResult};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0463_s21c";
const TASK_ID: &str = "neutral_task";
const STARTED_AT: u64 = 1_700_000_000_000;
const WIDTH: u32 = 160;
const HEIGHT: u32 = 90;
const GRAY: [u8; 3] = [60, 60, 60];
/// The colour of each screen's square.
const COLORS: [[u8; 3]; 4] = [[230, 20, 20], [20, 230, 20], [20, 20, 230], [230, 230, 20]];
/// Each screen's square, a colour mark's region.
const SQUARE: RecordRect = RecordRect {
    x: 0,
    y: 0,
    width: 16,
    height: 16,
};
/// The point each tap presses, on the gray.
const POINT: RecordPoint = RecordPoint { x: 100, y: 50 };
/// The self-checks of `record stop`, in the order they run.
const EVERY_CHECK: [&str; 7] = [
    "frames",
    "admission",
    "first_decision",
    "cross_check",
    "endings",
    "selects",
    "final_package",
];

// The PNG of screen `index`: its square of colour at the top left on gray.
fn screen(index: usize) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            pixels.extend(if x < 16 && y < 16 {
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

// What the command hands the recording for a performed input: no carrier package, no Runtime
// frames.
fn receipt() -> CommitClickRequest {
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

/// The fake Runtime's captures and the caller's clock: the screens its captures take, in order,
/// and the clock, which moves only by the waits.
struct Screens {
    shots: VecDeque<Vec<u8>>,
    taken: u32,
    now: Duration,
}

impl RecordFramePorts for Screens {
    fn wait(&mut self, duration: Duration) {
        self.now += duration;
    }

    fn elapsed(&mut self) -> Duration {
        self.now
    }

    fn capture(&mut self) -> LabResult<CapturedFrame> {
        let png = self
            .shots
            .pop_front()
            .expect("a capture the test did not script");
        self.taken += 1;
        Ok(CapturedFrame {
            frame: AttachFrameRequest {
                png,
                source: "lab_capture".to_owned(),
                runtime_artifact: None,
                capture_backend: Some("adb_screencap".to_owned()),
                freshness: None,
            },
            input_frame: Some(InputFrameReference {
                frame_id: serde_json::from_value(json!(format!("frame_{:032x}", self.taken)))
                    .expect("a frame id"),
                width: WIDTH,
                height: HEIGHT,
            }),
        })
    }
}

// A state directory with an active session of `INSTANCE` and its Lab recording, its game, server
// and locale known, without an OCR model. The lock is held as one command holds it and dropped
// before the directory.
struct Bench {
    lock: RecordingLock,
    ocr: LabOcr,
    temp: TempDir,
}

impl Bench {
    // The recording of four screens: three taps, each on the screen before it, the screen after it
    // one capture later, then each step marked with a colour mark on its square.
    fn marked() -> Self {
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
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_463_s21c", "Lab-CLI")
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
        let bench = Self {
            lock,
            ocr: LabOcr::from_vision_root(&temp.path().join("no-vision")),
            temp,
        };
        for index in 0..3 {
            bench.press(vec![screen(index), screen(index + 1)]);
        }
        for step in 1..=4 {
            bench.mark(step);
        }
        bench
    }

    fn state(&self) -> PathBuf {
        self.temp.path().join("state")
    }

    // `tap 100 50 --record`, the fake Runtime performing the input and its captures taking `shots`:
    // the screen before the press, then the screen after it one capture 800 ms later.
    fn press(&self, shots: Vec<Vec<u8>>) {
        let request = RecordPressRequest {
            click: PlanClickRequest::default(),
            point_press: Some(PointPress::Tap { point: POINT }),
            after: AfterCapture {
                still_ms: Some(0),
                ..AfterCapture::for_press(Some(800))
            },
        };
        let mut device = Screens {
            shots: shots.into(),
            taken: 0,
            now: Duration::ZERO,
        };
        let mut send = |_: &ClickPlan, _: InputFrameReference| -> LabResult<CommitClickRequest> {
            Ok(receipt())
        };
        record_press(&self.lock, &request, &mut send, &mut device).unwrap_or_else(|error| {
            panic!(
                "the press: {}: {} {:?}",
                error.code, error.message, error.details
            )
        });
    }

    // `record mark --step <step> --color square_<step>=0,0,16,16`.
    fn mark(&self, step: u32) {
        let request = MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            step: Some(step),
            add: vec![MarkSpec::region_only(
                format!("square_{step}"),
                MarkFamily::Color,
                SQUARE,
            )],
            ..MarkRequest::default()
        };
        record_mark(&self.lock, &request, false, &self.ocr).unwrap_or_else(|error| {
            panic!(
                "record mark: {}: {} {:?}",
                error.code, error.message, error.details
            )
        });
    }

    // The colour mark of step `step` with its member `member` set to `value`, written into the step
    // table in place.
    fn break_mark(&self, step: u32, member: &str, value: Value) {
        let path = recording_path(&self.state(), RECORD_ID);
        let mut recording: Value =
            serde_json::from_slice(&fs::read(&path).expect("step table")).expect("step table JSON");
        let entry = recording["steps"]
            .as_array_mut()
            .expect("steps")
            .iter_mut()
            .find(|entry| entry["index"] == step)
            .unwrap_or_else(|| panic!("no step {step}"));
        entry["marks"][0][member] = value;
        fs::write(
            &path,
            serde_json::to_vec(&recording).expect("step table JSON"),
        )
        .expect("step table");
    }

    // `record ready`, read as the command reads it: the instance's recording, no option.
    fn ready(&self) -> Value {
        let outcome = record_ready(
            &self.state(),
            INSTANCE,
            None,
            &RecordStopOptions::default(),
            &self.ocr,
        )
        .unwrap_or_else(|error| {
            panic!(
                "record ready: {}: {} {:?}",
                error.code, error.message, error.details
            )
        });
        serde_json::to_value(outcome).expect("the answer as JSON")
    }

    // The refusal of `record stop --dry-run`.
    fn stop_refusal(&self) -> LabError {
        record_stop(
            &self.lock,
            &RecordStopOptions {
                dry_run: true,
                ..RecordStopOptions::default()
            },
            &self.ocr,
        )
        .map(|outcome| serde_json::to_value(&outcome.lab).expect("lab JSON"))
        .expect_err("record stop refuses")
    }
}

// `checks` of the self-checks `ran` (each complete), in that order.
fn complete(ran: &[&str]) -> Value {
    json!(
        ran.iter()
            .map(|check| json!({"check": check, "complete": true}))
            .collect::<Vec<_>>()
    )
}

// The work order's first test: the cross-check fails on three frames.
#[test]
fn s21c_three_frames_that_fail_the_cross_check_are_three_items_and_stop_answers_the_first() {
    let bench = Bench::marked();
    for step in 2..=4 {
        bench.break_mark(step, "expected", json!([255, 255, 255]));
    }

    let ready = bench.ready();
    assert_eq!(ready["status"], "draft", "{ready}");
    assert_eq!(
        ready["missing"],
        json!([
            {"order": 1, "step": 2, "artifact_step": 2, "frame_id": "f0003",
             "what": "cross_check", "code": "record_step_self_mismatch",
             "details": {"page": "step_02", "frames": ["f0003"]}},
            {"order": 2, "step": 3, "artifact_step": 3, "frame_id": "f0005",
             "what": "cross_check", "code": "record_step_self_mismatch",
             "details": {"page": "step_03", "frames": ["f0005"]}},
            {"order": 3, "step": 4, "artifact_step": 4, "frame_id": null,
             "what": "cross_check", "code": "record_step_self_mismatch",
             "details": {"page": "step_04", "frames": ["f0006"]}}
        ]),
        "{ready}"
    );
    assert_eq!(ready["missing_total"], 3, "{ready}");
    // Every check before the final package ran to its end; the final package is built of what they
    // gave, so it did not run.
    assert_eq!(ready["checks"], complete(&EVERY_CHECK[..6]), "{ready}");
    assert_eq!(ready["checks_after"], json!(["final_package"]), "{ready}");

    // `record stop` answers the first failure with the code it answers today.
    let error = bench.stop_refusal();
    assert_eq!(error.code, "record_step_self_mismatch", "{}", error.message);
    let details = error.details.clone().expect("details");
    assert_eq!(details["record_index"], 2, "{details}");
    assert_eq!(details["frame_id"], "f0003", "{details}");
    assert_eq!(details["page"], "step_02", "{details}");
}

// The work order's second test: two self-checks fail, and both are listed in the order they run.
#[test]
fn s21c_two_self_checks_that_fail_are_listed_in_the_order_the_checks_run() {
    let bench = Bench::marked();
    // Step 1's page no longer passes on its frame: the first decision gives no click there, and the
    // cross-check finds the page does not match its own frame.
    bench.break_mark(1, "expected", json!([255, 255, 255]));

    let ready = bench.ready();
    assert_eq!(ready["status"], "draft", "{ready}");
    assert_eq!(
        ready["missing"],
        json!([
            {"order": 1, "step": 1, "artifact_step": 1, "frame_id": "f0001",
             "what": "first_decision", "code": "record_artifact_admission_failed",
             "details": {"frames": ["f0001"]}},
            {"order": 2, "step": 1, "artifact_step": 1, "frame_id": "f0001",
             "what": "cross_check", "code": "record_step_self_mismatch",
             "details": {"page": "step_01", "frames": ["f0001"]}}
        ]),
        "{ready}"
    );
    assert_eq!(ready["missing_total"], 2, "{ready}");
    assert_eq!(ready["checks"], complete(&EVERY_CHECK[..6]), "{ready}");
    assert_eq!(ready["checks_after"], json!(["final_package"]), "{ready}");

    // `record stop` answers the first, the first decision's refusal.
    let error = bench.stop_refusal();
    assert_eq!(
        error.code, "record_artifact_admission_failed",
        "{}",
        error.message
    );
    let details = error.details.clone().expect("details");
    assert_eq!(details["stage"], "first_decision", "{details}");
}

// The work order's third test: a check that must stop says so with its word and lists one.
#[test]
fn s21c_a_check_that_must_stop_says_so_and_lists_one() {
    let bench = Bench::marked();
    // A colour distance below 0, which the kernel's admission refuses for the package as a whole.
    bench.break_mark(2, "max_distance", json!(-1.0));

    let ready = bench.ready();
    assert_eq!(ready["status"], "draft", "{ready}");
    assert_eq!(
        ready["missing"],
        json!([
            {"order": 1, "step": null, "artifact_step": null, "frame_id": null,
             "what": "admission", "code": "record_artifact_admission_failed"}
        ]),
        "{ready}"
    );
    assert_eq!(ready["missing_total"], 1, "{ready}");
    assert_eq!(
        ready["checks"],
        json!([
            {"check": "frames", "complete": true},
            {"check": "admission", "complete": false, "reason": "whole_package"}
        ]),
        "{ready}"
    );
    assert_eq!(ready["checks_after"], json!(EVERY_CHECK[2..]), "{ready}");

    let error = bench.stop_refusal();
    assert_eq!(
        error.code, "record_artifact_admission_failed",
        "{}",
        error.message
    );
    let details = error.details.clone().expect("details");
    assert_eq!(details["stage"], "admission", "{details}");
}

// The work order's fourth test: a recording that passes.
#[test]
fn s21c_a_recording_that_passes_has_every_check_complete_and_is_ready() {
    let bench = Bench::marked();

    let ready = bench.ready();
    assert_eq!(ready["status"], "ready", "{ready}");
    assert_eq!(ready["missing"], json!([]), "{ready}");
    assert_eq!(ready["missing_total"], 0, "{ready}");
    assert_eq!(ready["checks"], complete(&EVERY_CHECK), "{ready}");
    assert_eq!(ready["checks_after"], json!([]), "{ready}");
}
