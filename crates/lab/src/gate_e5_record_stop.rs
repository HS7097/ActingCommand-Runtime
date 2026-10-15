// SPDX-License-Identifier: AGPL-3.0-only

// Test plan v2.2 E-5 G6 (exec key 35): `record stop` over a synthetic recording (EXEC-I5), as
// contracts/lab-recording.md states it: `generated_at_unix_ms` is the recording's
// `updated_at_unix_ms`, "so the same recording generates the same bytes again"; a page that does
// not match one of its own frames is `record_step_self_mismatch` and the recording stays active;
// `control.step_timeout_ms` is min(T, 60000) and a larger arrival timeout T only adds the warning
// `step_timeout_clamped` ("warnings never refuse"). Requirement basis:
// rd-1009/tests/req-basis-el.md section E-5, row "G6 record stop".
//
// EXEC-I5: the session file `record start` writes, then the Lab recording through the Lab's own
// commands, from two offline frames of 16x9 pixels: step 1 shows `home` (a red square at the
// top left) with a click, step 2 shows `list` (a blue square there). No OCR model is installed;
// the recording marks colours only.

use crate::recording::store::{old_record_path, recording_path};
use crate::{
    ClickSpec, LAB_RECORD_MARK_SCHEMA, LabOcr, MarkFamily, MarkRequest, MarkSpec, RecordRect,
    RecordStartOptions, RecordStopOptions, RecordStopOutcome, RecordingLock, record_mark,
    record_start, record_stop,
};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0001";
const TASK_ID: &str = "neutral_task";
const RED: [u8; 3] = [230, 20, 20];
const BLUE: [u8; 3] = [20, 20, 230];
const GRAY: [u8; 3] = [60, 60, 60];
const SQUARE: RecordRect = RecordRect {
    x: 0,
    y: 0,
    width: 4,
    height: 4,
};

// A 16x9 frame: a 4x4 square of `color` at the top left on gray.
fn frame_png(color: [u8; 3]) -> Vec<u8> {
    let mut pixels = Vec::new();
    for y in 0..9 {
        for x in 0..16 {
            pixels.extend(if x < 4 && y < 4 { color } else { GRAY });
        }
    }
    Frame::from_pixels(
        16,
        9,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
    .encode_png_fast()
    .expect("fixture PNG")
}

// A state directory with an active session of `INSTANCE` and its Lab recording of two steps.
// The lock is dropped before the directory.
struct Recorded {
    lock: RecordingLock,
    ocr: LabOcr,
    temp: TempDir,
}

impl Recorded {
    fn new() -> Self {
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
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_e5").expect("recording lock");
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
        let recorded = Self { lock, ocr, temp };
        recorded.mark("home", RED, true);
        recorded.mark("list", BLUE, false);
        recorded
    }

    // One offline frame showing `color`, its colour mark and, with `click`, the step's click.
    fn mark(&self, page: &str, color: [u8; 3], click: bool) {
        let frame = self.temp.path().join(format!("{page}.png"));
        fs::write(&frame, frame_png(color)).expect("frame file");
        let request = MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            frame: Some(frame.display().to_string()),
            add: vec![MarkSpec::region_only(
                format!("{page}_color"),
                MarkFamily::Color,
                SQUARE,
            )],
            click: click.then_some(ClickSpec {
                region: Some(RecordRect {
                    x: 8,
                    y: 2,
                    width: 4,
                    height: 4,
                }),
                from: None,
            }),
            ..MarkRequest::default()
        };
        record_mark(&self.lock, &request, false, &self.ocr).unwrap_or_else(|error| {
            panic!("record mark {page}: {}: {}", error.code, error.message)
        });
    }

    fn stop(&self, options: &RecordStopOptions) -> Result<Value, (String, Value)> {
        record_stop(&self.lock, options, &self.ocr)
            .map(|outcome| lab(&outcome))
            .map_err(|error| {
                (
                    error.code.clone(),
                    error.details.clone().unwrap_or(Value::Null),
                )
            })
    }

    fn recording_file(&self) -> PathBuf {
        recording_path(&self.temp.path().join("state"), RECORD_ID)
    }
}

// The `lab` field of the outcome with its status.
fn lab(outcome: &RecordStopOutcome) -> Value {
    let mut lab = serde_json::to_value(&outcome.lab).expect("lab JSON");
    lab["lab_status"] = json!(outcome.lab_status);
    lab
}

fn dry_run() -> RecordStopOptions {
    RecordStopOptions {
        dry_run: true,
        ..RecordStopOptions::default()
    }
}

// lab-recording.md: "`generated_at_unix_ms` is the recording's `updated_at_unix_ms`, so the same
// recording generates the same bytes again." Two dry runs and the generating run give one
// digest and one container sha256; the written package has those bytes; a stopped recording
// with its package is `already_generated`.
#[test]
fn gate_record_stop_the_same_recording_generates_the_same_bytes() {
    let recorded = Recorded::new();

    let first = recorded.stop(&dry_run()).expect("first dry run");
    let second = recorded.stop(&dry_run()).expect("second dry run");
    assert_eq!(first["lab_status"], "validated", "{first}");
    assert_eq!(first["digest"], second["digest"]);
    assert_eq!(first["sha256"], second["sha256"]);

    let generated = recorded
        .stop(&RecordStopOptions::default())
        .expect("record stop");
    assert_eq!(generated["lab_status"], "generated", "{generated}");
    assert_eq!(generated["digest"], first["digest"]);
    assert_eq!(generated["sha256"], first["sha256"]);
    let path = generated["path"].as_str().expect("package path");
    let written = Sha256::digest(fs::read(path).expect("written package"));
    assert_eq!(json!(format!("{written:x}")), generated["sha256"]);

    let again = recorded
        .stop(&RecordStopOptions::default())
        .expect("record stop of a stopped recording");
    assert_eq!(again["lab_status"], "already_generated", "{again}");
}

// lab-recording.md "Self-checks", cross-check "Self": "a page that does not match one of its own
// frames is `record_step_self_mismatch` (3) naming the frame"; a refusal leaves the recording
// active. The recording's `list` mark is edited to expect another colour than its own frame
// shows.
#[test]
fn gate_record_stop_refuses_a_page_that_does_not_match_its_own_frame() {
    let recorded = Recorded::new();
    let file = recorded.recording_file();
    let mut recording: Value =
        serde_json::from_slice(&fs::read(&file).expect("recording file")).expect("recording");
    let list = recording["steps"]
        .as_array_mut()
        .expect("steps")
        .iter_mut()
        .filter_map(|step| step["marks"].as_array_mut())
        .flatten()
        .find(|mark| mark["id"] == "list_color")
        .expect("the list mark");
    list["expected"] = json!([20, 230, 20]);
    fs::write(
        &file,
        serde_json::to_vec_pretty(&recording).expect("recording JSON"),
    )
    .expect("edited recording");

    let (code, details) = recorded
        .stop(&RecordStopOptions::default())
        .expect_err("a page that does not match its own frame is refused");

    assert_eq!(code, "record_step_self_mismatch", "{details}");
    assert!(
        details["failed"].as_array().is_some_and(|failed| failed
            .iter()
            .any(|id| id.as_str().is_some_and(|id| id.contains("list_color")))),
        "{details}"
    );
    let after: Value =
        serde_json::from_slice(&fs::read(file).expect("recording file")).expect("recording");
    assert_eq!(after["status"], "active");
    assert!(after["artifact"].is_null(), "{}", after["artifact"]);
}

// lab-recording.md "`control.step_timeout_ms` is min(T, 60000); a larger T adds the warning
// `step_timeout_clamped`", and warnings never refuse: an arrival timeout of 70 s generates the
// package with a step timeout of 60 s and the warning.
#[test]
fn gate_record_stop_a_clamped_step_timeout_is_a_warning_and_never_refuses() {
    let recorded = Recorded::new();

    let validated = recorded
        .stop(&RecordStopOptions {
            arrival_timeout_ms: Some(70_000),
            ..dry_run()
        })
        .expect("a clamped step timeout does not refuse");

    assert_eq!(validated["lab_status"], "validated", "{validated}");
    assert_eq!(validated["timeouts"]["arrival_timeout_ms"], 70_000);
    assert_eq!(validated["timeouts"]["step_timeout_ms"], 60_000);
    assert!(
        validated["warnings"]
            .as_array()
            .is_some_and(|warnings| warnings
                .iter()
                .any(|warning| warning["code"] == "step_timeout_clamped")),
        "{}",
        validated["warnings"]
    );
}
