// SPDX-License-Identifier: AGPL-3.0-only

// #463 S1-2 (contracts/lab-recording.md, "Fresh before, still after"; register §1.403): a press
// recorded by the command that makes it is recorded with a fresh screen before it and a still
// screen after it. `record_press` runs under the recording lock with a fake Runtime (its input
// answer, its captures and the caller's clock, `RecordFramePorts`); stillness is the Runtime's one
// frame-change measure.
// - `s12_an_animated_screen_after_is_kept_between_and_the_still_one_is_the_frame_after`: after the
//   wait of 800 ms the captures are an animation, the settled screen and the settled screen again
//   (one pixel differs): a capture every 300 ms until the third is still with the second. The
//   animation is kept as a superseded frame of step 2 (f0002), the third capture is the frame after
//   (f0003, live) and step 1's `after_frame`, the second is stored nowhere; `after_capture {800,
//   5000, true, 1400, 3, 1}`, no warning.
// - `s12_a_screen_never_still_is_taken_to_the_bound_and_warned`: for the bounds 0, 100, 300, 1000
//   and 5000 ms, captures that are never still with the one before: the waits are 800, then 300
//   each, the last only the rest of the bound; the captures are exactly ceil(bound / 300) + 2 with
//   the frame before (never more); the frame after is the last capture, at most three frames
//   between are kept superseded, `still` false, `waited_ms` 800 + the bound, and the answer's
//   `record.warnings` holds `after_not_still`.
// - `s12_an_old_frame_before_is_taken_again_and_stored_only_when_it_changed`: an open step's frame
//   recorded long ago: one capture before the press; still with it (one pixel differs), nothing is
//   stored and the press names the old frame (`refreshed` false); changed, it is stored, the old
//   frame superseded, and the press names the new one (`refreshed` true). A frame recorded at a time
//   the clock has not reached is young: no capture before the press.
// - `s12_a_step_with_marks_keeps_its_frame_before`: an open step with a mark (the older flow) and an
//   old frame: no capture before the press, which names the step's frame.
// Frames are synthetic 160x90 screens of one colour; no Runtime, no device; the tests read no clock
// (the age of a frame is written into the step table as a fixed time) and never sleep.

use crate::recording::store::{
    hex_sha256, lab_dir, old_record_path, recording_path, save_recording,
};
use crate::{
    AfterCapture, AttachFrameRequest, ClickEffect, ClickPlan, CommitClickRequest,
    LAB_RECORD_MARK_SCHEMA, LabOcr, LabRecording, MarkFamily, MarkRequest, MarkSpec, OpaqueJson,
    PlanClickRequest, PointPress, RecordFramePorts, RecordPoint, RecordPressOutcome,
    RecordPressRequest, RecordRect, RecordStartOptions, RecordingLock, record_attach_frame,
    record_mark, record_press, record_start,
};
use actingcommand_contract::LabResult;
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0463_s12";
const TASK_ID: &str = "neutral_task";
const STARTED_AT: u64 = 1_700_000_000_000;
const WIDTH: u32 = 160;
const HEIGHT: u32 = 90;
/// A time long before any run of these tests: a frame recorded then is old.
const LONG_AGO_UNIX_MS: u64 = STARTED_AT;
/// A time the clock has not reached (2100-01-01): a frame recorded then is younger than any bound.
const UNREACHED_UNIX_MS: u64 = 4_102_444_800_000;

/// The screens: the colour over the whole frame, and the colour of its top left pixel when it
/// differs. Screens of one colour are still with each other by the frame-change measure (one
/// pixel changes no cell of its digest) and differ in their bytes; screens of different colours
/// are not still.
const SCREENS: [([u8; 3], Option<[u8; 3]>); 6] = [
    ([40, 40, 200], None),
    ([40, 40, 200], Some([255, 255, 255])),
    ([200, 40, 40], None),
    ([40, 200, 40], None),
    ([40, 200, 40], Some([255, 255, 255])),
    ([200, 200, 40], None),
];
/// The screen a press is made on.
const BEFORE: usize = 0;
/// The same screen, taken again: still with [`BEFORE`].
const BEFORE_AGAIN: usize = 1;
/// A screen of an animation after the press.
const ANIMATION: usize = 2;
/// The screen the press led to.
const SETTLED: usize = 3;
/// The same screen, taken again: still with [`SETTLED`].
const SETTLED_AGAIN: usize = 4;
/// What the screen before became while the agent looked: not still with [`BEFORE`].
const CHANGED: usize = 5;

// The PNG of screen `index`.
fn screen(index: usize) -> Vec<u8> {
    let (color, corner) = SCREENS[index];
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            pixels.extend(match corner {
                Some(pixel) if x == 0 && y == 0 => pixel,
                _ => color,
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

// The sha256 of screen `index` as the recording stores it.
fn sha(index: usize) -> String {
    hex_sha256(&screen(index))
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

// What the command hands the recording for the input receipt: performed, no carrier package, no
// Runtime frames.
fn receipt() -> CommitClickRequest {
    CommitClickRequest {
        effect: ClickEffect::Performed,
        has_failure: false,
        carrier_package: None,
        req_id: opaque(json!("request-s12")),
        correlation_id: opaque(json!("request-s12-correlation")),
        action_id: opaque(json!("request-s12-action")),
        lease_id: None,
        failure: None,
        before: None,
        after: None,
    }
}

/// The fake Runtime's capture and the caller's clock: the screens its captures take, in order,
/// and what it was asked, in the log it shares with the press.
struct FakeDevice<'a> {
    screens: VecDeque<usize>,
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
        let index = self
            .screens
            .pop_front()
            .expect("a capture the test did not script");
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
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_463_s12", "Lab-CLI")
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

    // The live primary frame of step `step` as recorded at `at_unix_ms`, written as the recording
    // writes its step table: whether a press takes it again then depends on no clock.
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

    // `record mark --step <step> --color square=0,0,8,8`.
    fn mark(&self, step: u32) {
        let request = MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            step: Some(step),
            add: vec![MarkSpec::region_only(
                "state/square".to_owned(),
                MarkFamily::Color,
                RecordRect {
                    x: 0,
                    y: 0,
                    width: 8,
                    height: 8,
                },
            )],
            ..MarkRequest::default()
        };
        record_mark(&self.lock, &request, false, &self.ocr)
            .unwrap_or_else(|error| panic!("mark {step}: {}: {:?}", error.code, error.details));
    }

    // `record_press` of `request`, the fake Runtime performing the input and its captures taking
    // `screens`: the answer and the log of the fake (`capture`, `press`, `wait <ms>`).
    fn press(
        &self,
        request: &RecordPressRequest,
        screens: &[usize],
    ) -> (RecordPressOutcome, Vec<String>) {
        let log = RefCell::new(Vec::new());
        let result = {
            let mut device = FakeDevice {
                screens: screens.iter().copied().collect(),
                log: &log,
            };
            let mut press = |_plan: &ClickPlan| -> LabResult<CommitClickRequest> {
                log.borrow_mut().push("press".to_owned());
                Ok(receipt())
            };
            record_press(&self.lock, request, &mut press, &mut device)
        };
        let outcome = result.unwrap_or_else(|error| {
            panic!(
                "the press: {}: {} {:?}",
                error.code, error.message, error.details
            )
        });
        (outcome, log.into_inner())
    }

    fn recording(&self) -> Value {
        serde_json::from_slice(
            &fs::read(recording_path(&self.state(), RECORD_ID)).expect("step table"),
        )
        .expect("step table JSON")
    }

    // Whether the recording's frame store holds a file of sha256 `sha256`.
    fn stores(&self, sha256: &str) -> bool {
        lab_dir(&self.state(), RECORD_ID)
            .join("frames")
            .join(format!("{sha256}.png"))
            .exists()
    }
}

// `tap 40 30 --record`, the bound of the stillness test `still_max_ms` after the wait of 800 ms.
fn tap_request(still_max_ms: u64) -> RecordPressRequest {
    RecordPressRequest {
        click: PlanClickRequest::default(),
        point_press: Some(PointPress::Tap {
            point: RecordPoint { x: 40, y: 30 },
        }),
        after: AfterCapture {
            still_max_ms,
            ..AfterCapture::for_press(None)
        },
    }
}

// The frames of step `step` as (frame id, sha256, superseded).
fn frames_of(recording: &Value, step: u32) -> Vec<(String, String, bool)> {
    let found = recording["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .find(|item| item["index"] == step)
        .unwrap_or_else(|| panic!("step {step}: {recording}"));
    found["frames"]
        .as_array()
        .expect("frames")
        .iter()
        .map(|frame| {
            (
                frame["frame_id"].as_str().expect("frame id").to_owned(),
                frame["sha256"].as_str().expect("sha256").to_owned(),
                frame["superseded"].as_bool().expect("superseded"),
            )
        })
        .collect()
}

fn entry(frame_id: &str, sha256: String, superseded: bool) -> (String, String, bool) {
    (frame_id.to_owned(), sha256, superseded)
}

#[test]
fn s12_an_animated_screen_after_is_kept_between_and_the_still_one_is_the_frame_after() {
    let bench = Bench::new();
    let (outcome, log) = bench.press(
        &tap_request(5_000),
        &[BEFORE, ANIMATION, SETTLED, SETTLED_AGAIN],
    );
    assert_eq!(
        log,
        [
            "capture", "press", "wait 800", "capture", "wait 300", "capture", "wait 300", "capture"
        ]
    );

    let recording = bench.recording();
    assert_eq!(
        frames_of(&recording, 1),
        [entry("f0001", sha(BEFORE), false)],
        "{recording}"
    );
    // The animation is a superseded frame of step 2; the third capture is its live frame.
    assert_eq!(
        frames_of(&recording, 2),
        [
            entry("f0002", sha(ANIMATION), true),
            entry("f0003", sha(SETTLED_AGAIN), false)
        ],
        "{recording}"
    );
    // The second capture, still with the third, is stored nowhere.
    assert!(
        !recording.to_string().contains(&sha(SETTLED)),
        "{recording}"
    );
    assert!(!bench.stores(&sha(SETTLED)));
    let execution = &recording["steps"][0]["click"]["execution"];
    assert_eq!(execution["before_frame"], "f0001", "{recording}");
    assert_eq!(execution["after_frame"], "f0003", "{recording}");
    let taken = json!({
        "after_ms": 800,
        "still_max_ms": 5000,
        "still": true,
        "waited_ms": 1400,
        "captures": 3,
        "kept": 1
    });
    assert_eq!(execution["after_capture"], taken, "{recording}");

    let record = serde_json::to_value(&outcome).expect("the record");
    assert_eq!(record["status"], "press_recorded", "{record}");
    assert_eq!(
        record["click"]["execution"]["after_frame"], "f0003",
        "{record}"
    );
    let after = &record["after"];
    assert_eq!(after["step"], 2, "{record}");
    assert_eq!(after["frame_id"], "f0003", "{record}");
    assert_eq!(after["sha256"], sha(SETTLED_AGAIN), "{record}");
    for (member, value) in taken.as_object().expect("the record of the frame after") {
        assert_eq!(after[member], *value, "{member}: {record}");
    }
    assert_eq!(record["warnings"], json!([]), "{record}");
}

#[test]
fn s12_a_screen_never_still_is_taken_to_the_bound_and_warned() {
    for bound in [0_u64, 100, 300, 1_000, 5_000] {
        let bench = Bench::new();
        // The captures after the press: one after the wait, then one per pause of the bound.
        let after_captures = 1 + bound.div_ceil(300);
        let mut screens = vec![BEFORE];
        screens.extend((0..after_captures).map(
            |index| {
                if index % 2 == 0 { ANIMATION } else { SETTLED }
            },
        ));
        let (outcome, log) = bench.press(&tap_request(bound), &screens);

        let mut pauses = vec![300_u64; (bound / 300) as usize];
        if bound % 300 > 0 {
            pauses.push(bound % 300);
        }
        let mut expected = vec!["capture".to_owned(), "press".to_owned()];
        expected.extend(["wait 800".to_owned(), "capture".to_owned()]);
        for pause in &pauses {
            expected.extend([format!("wait {pause}"), "capture".to_owned()]);
        }
        assert_eq!(log, expected, "bound {bound}");
        // The frame before and the captures after: the bound of the design, ceil(bound / 300) + 2,
        // reached and never passed.
        let captures = log.iter().filter(|line| *line == "capture").count() as u64;
        assert_eq!(captures, bound.div_ceil(300) + 2, "bound {bound}");

        // The frames between kept (at most three, the first ones) and the last capture, live.
        let kept = (after_captures - 1).min(3);
        let last = screens[screens.len() - 1];
        let recording = bench.recording();
        let mut between = (0..kept)
            .map(|index| {
                let id = format!("f{:04}", 2 + index);
                entry(&id, sha(screens[1 + index as usize]), true)
            })
            .collect::<Vec<_>>();
        let after_id = format!("f{:04}", 2 + kept);
        between.push(entry(&after_id, sha(last), false));
        assert_eq!(
            frames_of(&recording, 2),
            between,
            "bound {bound}: {recording}"
        );
        let execution = &recording["steps"][0]["click"]["execution"];
        assert_eq!(
            execution["after_frame"], after_id,
            "bound {bound}: {recording}"
        );
        let taken = json!({
            "after_ms": 800,
            "still_max_ms": bound,
            "still": false,
            "waited_ms": 800 + bound,
            "captures": after_captures,
            "kept": kept
        });
        assert_eq!(
            execution["after_capture"], taken,
            "bound {bound}: {recording}"
        );

        let record = serde_json::to_value(&outcome).expect("the record");
        assert_eq!(
            record["after"]["frame_id"], after_id,
            "bound {bound}: {record}"
        );
        assert_eq!(
            record["after"]["sha256"],
            sha(last),
            "bound {bound}: {record}"
        );
        assert_eq!(record["after"]["still"], false, "bound {bound}: {record}");
        assert_eq!(
            record["warnings"],
            json!(["after_not_still"]),
            "bound {bound}: {record}"
        );
    }
}

#[test]
fn s12_an_old_frame_before_is_taken_again_and_stored_only_when_it_changed() {
    // Still with the old frame: nothing stored, the press names the old frame.
    {
        let bench = Bench::new();
        bench.capture(BEFORE);
        bench.recorded_at(1, LONG_AGO_UNIX_MS);
        let (outcome, log) = bench.press(&tap_request(5_000), &[BEFORE_AGAIN, SETTLED, SETTLED]);
        assert_eq!(
            log,
            [
                "capture", "press", "wait 800", "capture", "wait 300", "capture"
            ]
        );
        let recording = bench.recording();
        assert_eq!(
            frames_of(&recording, 1),
            [entry("f0001", sha(BEFORE), false)],
            "{recording}"
        );
        assert!(
            !recording.to_string().contains(&sha(BEFORE_AGAIN)),
            "{recording}"
        );
        assert!(!bench.stores(&sha(BEFORE_AGAIN)));
        let execution = &recording["steps"][0]["click"]["execution"];
        assert_eq!(execution["before_frame"], "f0001", "{recording}");
        assert_eq!(execution["after_frame"], "f0002", "{recording}");
        assert_eq!(outcome.before.frame_id, "f0001");
        assert_eq!(outcome.before.sha256, sha(BEFORE));
        assert!(!outcome.before.refreshed);
    }

    // Changed: the new frame is the step's live frame, the old one superseded, and the press
    // names the new one.
    {
        let bench = Bench::new();
        bench.capture(BEFORE);
        bench.recorded_at(1, LONG_AGO_UNIX_MS);
        let (outcome, log) = bench.press(&tap_request(5_000), &[CHANGED, SETTLED, SETTLED]);
        assert_eq!(
            log,
            [
                "capture", "press", "wait 800", "capture", "wait 300", "capture"
            ]
        );
        let recording = bench.recording();
        assert_eq!(
            frames_of(&recording, 1),
            [
                entry("f0001", sha(BEFORE), true),
                entry("f0002", sha(CHANGED), false)
            ],
            "{recording}"
        );
        let execution = &recording["steps"][0]["click"]["execution"];
        assert_eq!(execution["before_frame"], "f0002", "{recording}");
        assert_eq!(execution["after_frame"], "f0003", "{recording}");
        assert_eq!(outcome.before.frame_id, "f0002");
        assert_eq!(outcome.before.sha256, sha(CHANGED));
        assert!(outcome.before.refreshed);
    }

    // Young: nothing is taken before the press.
    {
        let bench = Bench::new();
        bench.capture(BEFORE);
        bench.recorded_at(1, UNREACHED_UNIX_MS);
        let (outcome, log) = bench.press(&tap_request(5_000), &[SETTLED, SETTLED]);
        assert_eq!(log, ["press", "wait 800", "capture", "wait 300", "capture"]);
        let recording = bench.recording();
        assert_eq!(
            frames_of(&recording, 1),
            [entry("f0001", sha(BEFORE), false)],
            "{recording}"
        );
        assert_eq!(outcome.before.frame_id, "f0001");
        assert!(!outcome.before.refreshed);
    }
}

#[test]
fn s12_a_step_with_marks_keeps_its_frame_before() {
    let bench = Bench::new();
    bench.capture(BEFORE);
    bench.mark(1);
    bench.recorded_at(1, LONG_AGO_UNIX_MS);
    let (outcome, log) = bench.press(&tap_request(5_000), &[SETTLED, SETTLED]);
    // No capture before the press.
    assert_eq!(log, ["press", "wait 800", "capture", "wait 300", "capture"]);
    assert_eq!(outcome.click.status, "press_recorded");
    assert_eq!(outcome.before.frame_id, "f0001");
    assert!(!outcome.before.refreshed);
    let recording = bench.recording();
    assert_eq!(
        frames_of(&recording, 1),
        [entry("f0001", sha(BEFORE), false)],
        "{recording}"
    );
    let step = &recording["steps"][0];
    assert!(
        !step["marks"].as_array().expect("marks").is_empty(),
        "{step}"
    );
    assert_eq!(
        step["click"]["execution"]["before_frame"], "f0001",
        "{step}"
    );
}
