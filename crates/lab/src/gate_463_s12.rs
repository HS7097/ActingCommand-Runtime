// SPDX-License-Identifier: AGPL-3.0-only

// #463 S1-2 (contracts/lab-recording.md, "Recording while playing", "The screen after"; register
// §1.403): after a press recorded by the command that makes it, the screen is captured until it is
// still and every capture is kept. `record_press` runs under the recording lock with a fake
// Runtime (its input answer, its captures and the caller's clock, `RecordFramePorts`): the fake's
// clock moves only by its waits and by the time a capture is scripted to take, and the tests read
// no clock. Stillness is the Runtime's one frame-change measure. The defaults: the first capture
// 100 ms after the press, one every 100 ms, at most 5000 ms. Fix round 3 (the coordinator's
// ruling): the screen is still when it has not changed for at least `still_ms` (300): the newest
// capture and the unbroken run of captures before it within the bound of the newest hold at least
// one earlier capture, and the newest started at least `still_ms` after the run's first, however
// long a capture takes.
// - `s12_an_animation_that_settles_keeps_every_capture_and_the_last_is_the_frame_after`: captures
//   every 100 ms: after the press two frames of an animation, then the settled screen four times
//   (two of them one pixel off, still by the measure): four unchanged captures are needed, so six
//   captures, the sixth the first that started 300 ms after the first of its run (three unchanged
//   do not end the loop). All six are frames of step 2 in order (f0002 to f0007), the first five
//   superseded, f0007 live and step 1's `after_frame`; `after_capture {after_ms: 100,
//   interval_ms: 100, still_ms: 300, still_max_ms: 5000, still: true, waited_ms: 600, captures: 6,
//   kept: 5}`, the answer's `after` the same, no warning.
// - `s12_captures_of_400_ms_are_still_after_two_unchanged`: every capture takes 400 ms, longer than
//   `still_ms`: after a changed first capture, two unchanged ones suffice (the second starts 400 ms
//   after the first of its run): three captures, each at once after the one before, `waited_ms`
//   900, `still` true.
// - `s12_a_change_in_the_middle_of_a_run_starts_it_again`: every 100 ms, the settled screen three
//   times, one other screen, then the settled screen four times: the change breaks the run, and the
//   settled screen that comes back before it does not count; still at the eighth capture,
//   `waited_ms` 800.
// - `s12_a_screen_that_never_settles_is_taken_to_the_bound_and_warned`: for the bound and interval
//   (0, 100), (250, 100), (300, 100), (1000, 50), (5000, 100) and (12000, 100), screens that never
//   settle: exactly 1 + bound / interval captures (121 at the last), one wait of the interval
//   before each but the first, every capture a frame of step 2 and the last the frame after,
//   `still` false, `waited_ms` 100 + (captures - 1) x interval, `kept` captures - 1, and the
//   answer's `record.warnings` `["after_not_still"]`.
// - `s12_a_clock_that_does_not_advance_ends_the_loop_by_count`: a caller's clock that stands still
//   (its waits and captures move it not at all), bound 1000 ms at 100: the loop ends by count after
//   exactly 11 captures of one screen (never still: no time passed), `still` false, `waited_ms` 0,
//   the warning; a twelfth capture would fail the test.
// - `s12_a_capture_slower_than_the_interval_is_followed_at_once`: the first capture after the press
//   takes 250 ms: the next starts at once (no wait), the ones after it 100 ms apart; the fifth,
//   at 650, is the first that started 300 ms after the first of its run (at 350): `waited_ms` 650,
//   five captures.
// - `s12_identical_captures_are_one_file_and_several_entries`: the same screen four times after the
//   press: four frames of step 2 (f0002 to f0005) of one sha256 and one path, each its own entry with
//   its own time, and two files in the frame store (the screen before and this one).
// - `s12_two_presses_keep_the_after_loop_then_the_press_frame_in_order`: two taps in a row: step 2
//   holds, in time order, the five captures of the first tap's after loop (superseded), then the
//   capture the second tap was sent on (f0007, live); step 1's `after_frame` stays f0006, the loop's
//   last, which the second tap's capture does not rewrite; step 2's press names f0007 before and
//   f0011, its own loop's last, after.
// - `s12_still_ms_0_takes_one_capture_untested`: `--still-ms 0`: one capture 100 ms after the press
//   is the frame after, `still` null, no warning.
// - `s12_a_capture_that_fails_midway_keeps_what_was_taken`: the third capture after the press
//   fails: the two taken are frames of step 2 (f0002 superseded, f0003 live), step 1's
//   `after_frame` f0003, `after_capture {100, 100, 300, 5000, still: null, waited_ms: 200,
//   captures: 2, kept: 1, unfinished: true}`; the answer is the capture's own `device_error`,
//   its details `record` (the press with that after frame) and `after: {after_ms: 100, taken:
//   true, captures: 2}`.
// - `s12_each_parameter_out_of_its_range_is_refused_before_anything`: `interval_ms` 19 and 1001,
//   `still_ms` 5001, `still_max_ms` 60001: `validation_failed` `details{argument, reason: "range",
//   value, min, max}`: nothing captured, pressed or written.
// - `s12_more_than_120_intervals_in_the_bound_are_refused_before_anything`: a bound of more than 120
//   intervals (12001 ms at 100, 60000 at 499, 2401 at 20) is out of the range the interval allows,
//   `validation_failed` `details{argument: "still_max_ms", reason: "range", value, min: 0, max: 120
//   x interval_ms, interval_ms}` (fix round 3: the range word): nothing captured, pressed or
//   written.
// Frames are synthetic 160x90 screens of one colour; no Runtime, no device; nothing sleeps.

use crate::recording::store::{hex_sha256, lab_dir, old_record_path, recording_path};
use crate::{
    AfterCapture, AttachFrameRequest, CapturedFrame, ClickEffect, ClickPlan, CommitClickRequest,
    OpaqueJson, PlanClickRequest, PointPress, RecordFramePorts, RecordPoint, RecordPressOutcome,
    RecordPressRequest, RecordStartOptions, RecordingLock, record_attach_frame, record_press,
    record_start,
};
use actingcommand_contract::{InputFrameReference, LabError, LabResult};
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

/// The screens: the colour over the whole frame, and the colour of its top left pixel when it
/// differs. Screens of one colour are still with each other by the frame-change measure (one pixel
/// changes no cell of its digest) and differ in their bytes; screens of different colours are not
/// still.
const SCREENS: [([u8; 3], Option<[u8; 3]>); 6] = [
    ([40, 40, 200], None),
    ([200, 40, 40], None),
    ([200, 200, 40], None),
    ([40, 200, 40], None),
    ([40, 200, 40], Some([255, 255, 255])),
    ([200, 40, 200], None),
];
/// The screen a press is made on.
const BEFORE: usize = 0;
/// A frame of an animation after the press.
const ANIMATION: usize = 1;
/// A later frame of that animation.
const ANIMATION_LATER: usize = 2;
/// The screen the press led to.
const SETTLED: usize = 3;
/// The same screen with one pixel off: other bytes, still with [`SETTLED`].
const SETTLED_AGAIN: usize = 4;
/// The screen the next press led to.
const NEXT: usize = 5;

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

// The reference of the frame the fake Runtime committed for a capture, of the screens' size.
fn reference() -> InputFrameReference {
    InputFrameReference {
        frame_id: serde_json::from_value(json!(format!("frame_{:032x}", 1))).expect("a frame id"),
        width: WIDTH,
        height: HEIGHT,
    }
}

/// One capture of the fake Runtime: the screen it takes (`None`: the capture fails) and how long it
/// takes on the fake clock.
#[derive(Clone, Copy, Debug)]
struct Shot {
    screen: Option<usize>,
    takes_ms: u64,
}

// A capture of `screen` that takes no time.
const fn shot(screen: usize) -> Shot {
    slow(screen, 0)
}

// A capture of `screen` that takes `takes_ms` on the fake clock.
const fn slow(screen: usize, takes_ms: u64) -> Shot {
    Shot {
        screen: Some(screen),
        takes_ms,
    }
}

/// A capture the fake Runtime fails with a device error.
const FAILED: Shot = Shot {
    screen: None,
    takes_ms: 0,
};

/// The fake Runtime's capture and the caller's clock: the shots its captures take, in order, the
/// log it shares with the press (`capture`, `press`, `wait <ms>`), and the clock, which moves only
/// by the waits and by the time each capture takes, or not at all when it is `frozen`.
struct FakeDevice<'a> {
    shots: VecDeque<Shot>,
    log: &'a RefCell<Vec<String>>,
    now: Duration,
    frozen: bool,
}

impl RecordFramePorts for FakeDevice<'_> {
    fn wait(&mut self, duration: Duration) {
        if !self.frozen {
            self.now += duration;
        }
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
        if !self.frozen {
            self.now += Duration::from_millis(shot.takes_ms);
        }
        let screen = shot
            .screen
            .ok_or_else(|| LabError::device("the fake Runtime's capture failed"))?;
        Ok(CapturedFrame {
            frame: captured(screen),
            input_frame: Some(reference()),
        })
    }

    fn elapsed(&mut self) -> Duration {
        self.now
    }
}

// A state directory with an active session of `INSTANCE` (the header `record start` writes) and its
// Lab recording, before any step. The lock is held as one command holds it and dropped before the
// directory.
struct Bench {
    lock: RecordingLock,
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
        Self { lock, temp }
    }

    fn state(&self) -> PathBuf {
        self.temp.path().join("state")
    }

    // `capture --record` of screen `index`.
    fn capture(&self, index: usize) {
        record_attach_frame(&self.lock, captured(index))
            .unwrap_or_else(|error| panic!("frame {index}: {}: {}", error.code, error.message));
    }

    // `record_press` of `request`, the fake Runtime performing the input and its captures taking
    // `shots`: the result and the log of the fake.
    fn try_press(
        &self,
        request: &RecordPressRequest,
        shots: &[Shot],
    ) -> (LabResult<RecordPressOutcome>, Vec<String>) {
        self.try_press_on(request, shots, false)
    }

    // As [`Bench::try_press`], on a clock that stands still when `frozen`.
    fn try_press_on(
        &self,
        request: &RecordPressRequest,
        shots: &[Shot],
        frozen: bool,
    ) -> (LabResult<RecordPressOutcome>, Vec<String>) {
        let log = RefCell::new(Vec::new());
        let result = {
            let mut device = FakeDevice {
                shots: shots.iter().copied().collect(),
                log: &log,
                now: Duration::ZERO,
                frozen,
            };
            let mut press =
                |_plan: &ClickPlan, _frame: InputFrameReference| -> LabResult<CommitClickRequest> {
                    log.borrow_mut().push("press".to_owned());
                    Ok(receipt())
                };
            record_press(&self.lock, request, &mut press, &mut device)
        };
        (result, log.into_inner())
    }

    // As [`Bench::try_press`], the press performed and recorded, or the test's panic.
    fn press(
        &self,
        request: &RecordPressRequest,
        shots: &[Shot],
    ) -> (RecordPressOutcome, Vec<String>) {
        let (result, log) = self.try_press(request, shots);
        let outcome = result.unwrap_or_else(|error: LabError| {
            panic!(
                "the press: {}: {} {:?}",
                error.code, error.message, error.details
            )
        });
        (outcome, log)
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
        let mut files = Vec::new();
        let mut pending = vec![(lab_dir(&self.state(), RECORD_ID), String::new())];
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

    // The names of the files of the recording's frame store.
    fn frame_files(&self) -> Vec<String> {
        let dir = lab_dir(&self.state(), RECORD_ID).join("frames");
        let mut names = fs::read_dir(dir)
            .expect("the frame store")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        names.sort();
        names
    }
}

// `tap 40 30 --record`, the screen after it taken as `after` says.
fn tap_request(after: AfterCapture) -> RecordPressRequest {
    RecordPressRequest {
        click: PlanClickRequest::default(),
        point_press: Some(PointPress::Tap {
            point: RecordPoint { x: 40, y: 30 },
        }),
        after,
    }
}

// The defaults: `tap 40 30 --record`.
fn default_tap() -> RecordPressRequest {
    tap_request(AfterCapture::for_press(None))
}

// The log of a press on an open or new recording whose after loop takes `captures` captures with
// no capture taking time: the capture before, the press, the wait of 100 ms, the first capture,
// then a wait of `interval_ms` before each other one.
fn press_log(captures: u64, interval_ms: u64) -> Vec<String> {
    let mut log = ["capture", "press", "wait 100", "capture"]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for _ in 1..captures {
        log.push(format!("wait {interval_ms}"));
        log.push("capture".to_owned());
    }
    log
}

// The frames of step `step` as (frame id, sha256, superseded).
fn frames_of(recording: &Value, step: u32) -> Vec<(String, String, bool)> {
    step_of(recording, step)["frames"]
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

fn step_of(recording: &Value, step: u32) -> &Value {
    recording["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .find(|item| item["index"] == step)
        .unwrap_or_else(|| panic!("step {step}: {recording}"))
}

// The frames of an after loop: frame ids from `first_id` on, one per screen, every one superseded
// but the last.
fn loop_frames(first_id: u32, screens: &[usize]) -> Vec<(String, String, bool)> {
    screens
        .iter()
        .enumerate()
        .map(|(index, screen)| {
            (
                format!("f{:04}", first_id + index as u32),
                sha(*screen),
                index + 1 < screens.len(),
            )
        })
        .collect()
}

#[test]
fn s12_an_animation_that_settles_keeps_every_capture_and_the_last_is_the_frame_after() {
    let bench = Bench::new();
    let after = [
        ANIMATION,
        ANIMATION_LATER,
        SETTLED,
        SETTLED_AGAIN,
        SETTLED,
        SETTLED_AGAIN,
    ];
    let mut shots = vec![shot(BEFORE)];
    shots.extend(after.map(shot));
    let (outcome, log) = bench.press(&default_tap(), &shots);
    // The captures start 100, 200, ..., 600 ms after the press. The settled screen's run begins at
    // 300: at 500 (three unchanged) the newest started 200 ms after it, at 600 (four) 300 ms.
    assert_eq!(log, press_log(6, 100));

    let recording = bench.recording();
    assert_eq!(
        frames_of(&recording, 1),
        [("f0001".to_owned(), sha(BEFORE), false)],
        "{recording}"
    );
    // Every capture is kept, in order; all but the last superseded.
    assert_eq!(
        frames_of(&recording, 2),
        loop_frames(2, &after),
        "{recording}"
    );
    let execution = &step_of(&recording, 1)["click"]["execution"];
    assert_eq!(execution["before_frame"], "f0001", "{recording}");
    assert_eq!(execution["after_frame"], "f0007", "{recording}");
    let taken = json!({
        "after_ms": 100,
        "interval_ms": 100,
        "still_ms": 300,
        "still_max_ms": 5000,
        "still": true,
        "waited_ms": 600,
        "captures": 6,
        "kept": 5
    });
    assert_eq!(execution["after_capture"], taken, "{recording}");

    let record = serde_json::to_value(&outcome).expect("the record");
    assert_eq!(record["status"], "press_recorded", "{record}");
    assert_eq!(
        record["click"]["execution"]["after_frame"], "f0007",
        "{record}"
    );
    let answered = &record["after"];
    assert_eq!(answered["step"], 2, "{record}");
    assert_eq!(answered["frame_id"], "f0007", "{record}");
    assert_eq!(answered["sha256"], sha(SETTLED_AGAIN), "{record}");
    for (member, value) in taken.as_object().expect("the record of the frame after") {
        assert_eq!(answered[member], *value, "{member}: {record}");
    }
    assert_eq!(record["warnings"], json!([]), "{record}");
}

#[test]
fn s12_captures_of_400_ms_are_still_after_two_unchanged() {
    let bench = Bench::new();
    let after = [ANIMATION, SETTLED, SETTLED_AGAIN];
    let mut shots = vec![slow(BEFORE, 400)];
    shots.extend(after.map(|screen| slow(screen, 400)));
    let (outcome, log) = bench.press(&default_tap(), &shots);
    // The press ends at 400; the captures start at 500, 900 and 1300, each at once after the one
    // before it. At 1300 the run of the settled screen began at 900, 400 ms earlier: still.
    assert_eq!(
        log,
        [
            "capture", "press", "wait 100", "capture", "capture", "capture"
        ]
    );
    let recording = bench.recording();
    assert_eq!(
        frames_of(&recording, 2),
        loop_frames(2, &after),
        "{recording}"
    );
    let execution = &step_of(&recording, 1)["click"]["execution"];
    assert_eq!(execution["after_frame"], "f0004", "{recording}");
    assert_eq!(
        execution["after_capture"],
        json!({
            "after_ms": 100,
            "interval_ms": 100,
            "still_ms": 300,
            "still_max_ms": 5000,
            "still": true,
            "waited_ms": 900,
            "captures": 3,
            "kept": 2
        }),
        "{recording}"
    );
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
}

#[test]
fn s12_a_change_in_the_middle_of_a_run_starts_it_again() {
    let bench = Bench::new();
    let after = [
        SETTLED,
        SETTLED_AGAIN,
        SETTLED,
        ANIMATION,
        SETTLED,
        SETTLED_AGAIN,
        SETTLED,
        SETTLED_AGAIN,
    ];
    let mut shots = vec![shot(BEFORE)];
    shots.extend(after.map(shot));
    let (outcome, log) = bench.press(&default_tap(), &shots);
    // 100 to 300 the settled screen, 400 another, 500 to 800 the settled screen again: the run of
    // the newest stops at 400, so the settled screen before it does not count (at 500 it would
    // reach back to 100); still at 800, 300 ms after 500.
    assert_eq!(log, press_log(8, 100));
    let recording = bench.recording();
    assert_eq!(
        frames_of(&recording, 2),
        loop_frames(2, &after),
        "{recording}"
    );
    let execution = &step_of(&recording, 1)["click"]["execution"];
    assert_eq!(execution["after_frame"], "f0009", "{recording}");
    assert_eq!(execution["after_capture"]["still"], true, "{recording}");
    assert_eq!(execution["after_capture"]["waited_ms"], 800, "{recording}");
    assert_eq!(execution["after_capture"]["captures"], 8, "{recording}");
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
}

#[test]
fn s12_a_screen_that_never_settles_is_taken_to_the_bound_and_warned() {
    for (still_max_ms, interval_ms) in [
        (0_u64, 100_u64),
        (250, 100),
        (300, 100),
        (1_000, 50),
        (5_000, 100),
        (12_000, 100),
    ] {
        let case = format!("still_max_ms {still_max_ms}, interval_ms {interval_ms}");
        let bench = Bench::new();
        let captures = 1 + still_max_ms / interval_ms;
        let after = (0..captures)
            .map(|index| if index % 2 == 0 { ANIMATION } else { SETTLED })
            .collect::<Vec<_>>();
        let mut shots = vec![shot(BEFORE)];
        shots.extend(after.iter().copied().map(shot));
        let request = tap_request(AfterCapture {
            interval_ms: Some(interval_ms),
            still_max_ms,
            ..AfterCapture::for_press(None)
        });
        let (outcome, log) = bench.press(&request, &shots);
        // Exactly 1 + bound / interval captures after the press, the bound reached and never passed.
        assert_eq!(log, press_log(captures, interval_ms), "{case}");

        let recording = bench.recording();
        assert_eq!(
            frames_of(&recording, 2),
            loop_frames(2, &after),
            "{case}: {recording}"
        );
        let after_id = format!("f{:04}", 1 + captures);
        let execution = &step_of(&recording, 1)["click"]["execution"];
        assert_eq!(execution["after_frame"], after_id, "{case}: {recording}");
        let taken = json!({
            "after_ms": 100,
            "interval_ms": interval_ms,
            "still_ms": 300,
            "still_max_ms": still_max_ms,
            "still": false,
            "waited_ms": 100 + (captures - 1) * interval_ms,
            "captures": captures,
            "kept": captures - 1
        });
        assert_eq!(execution["after_capture"], taken, "{case}: {recording}");

        let record = serde_json::to_value(&outcome).expect("the record");
        assert_eq!(record["after"]["frame_id"], after_id, "{case}: {record}");
        assert_eq!(record["after"]["still"], false, "{case}: {record}");
        assert_eq!(
            record["warnings"],
            json!(["after_not_still"]),
            "{case}: {record}"
        );
    }
}

#[test]
fn s12_a_clock_that_does_not_advance_ends_the_loop_by_count() {
    let bench = Bench::new();
    let after = [SETTLED; 11];
    let mut shots = vec![shot(BEFORE)];
    shots.extend(after.map(shot));
    let request = tap_request(AfterCapture {
        still_max_ms: 1_000,
        ..AfterCapture::for_press(None)
    });
    // The fake's clock stands still: every wait asks for the interval and none passes. A twelfth
    // capture would find no scripted shot and fail the test instead of holding it.
    let (result, log) = bench.try_press_on(&request, &shots, true);
    let outcome = result.expect("the press on a clock that stands still");
    assert_eq!(log, press_log(11, 100));

    let recording = bench.recording();
    assert_eq!(
        frames_of(&recording, 2),
        loop_frames(2, &after),
        "{recording}"
    );
    assert_eq!(
        step_of(&recording, 1)["click"]["execution"]["after_capture"],
        json!({
            "after_ms": 100,
            "interval_ms": 100,
            "still_ms": 300,
            "still_max_ms": 1000,
            "still": false,
            "waited_ms": 0,
            "captures": 11,
            "kept": 10
        }),
        "{recording}"
    );
    assert_eq!(outcome.warnings, ["after_not_still"]);
}

#[test]
fn s12_a_capture_slower_than_the_interval_is_followed_at_once() {
    let bench = Bench::new();
    let after = [ANIMATION, SETTLED, SETTLED_AGAIN, SETTLED, SETTLED_AGAIN];
    let mut shots = vec![shot(BEFORE), slow(ANIMATION, 250)];
    shots.extend(after[1..].iter().copied().map(shot));
    let (outcome, log) = bench.press(&default_tap(), &shots);
    // The first capture after the press starts at 100 ms and ends at 350: the second starts at once,
    // the others 100 ms apart, at 450, 550 and 650; the settled screen's run begins at 350, so the
    // fifth, 300 ms after it, is the first still one.
    assert_eq!(
        log,
        [
            "capture", "press", "wait 100", "capture", "capture", "wait 100", "capture",
            "wait 100", "capture", "wait 100", "capture"
        ]
    );
    let recording = bench.recording();
    assert_eq!(
        frames_of(&recording, 2),
        loop_frames(2, &after),
        "{recording}"
    );
    let taken = json!({
        "after_ms": 100,
        "interval_ms": 100,
        "still_ms": 300,
        "still_max_ms": 5000,
        "still": true,
        "waited_ms": 650,
        "captures": 5,
        "kept": 4
    });
    assert_eq!(
        step_of(&recording, 1)["click"]["execution"]["after_capture"],
        taken,
        "{recording}"
    );
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
}

#[test]
fn s12_identical_captures_are_one_file_and_several_entries() {
    let bench = Bench::new();
    let after = [SETTLED; 4];
    let mut shots = vec![shot(BEFORE)];
    shots.extend(after.map(shot));
    let (_, log) = bench.press(&default_tap(), &shots);
    assert_eq!(log, press_log(4, 100));

    let recording = bench.recording();
    assert_eq!(
        frames_of(&recording, 2),
        loop_frames(2, &after),
        "{recording}"
    );
    // One file: every entry names the same path; each entry is its own, with its own time.
    let frames = step_of(&recording, 2)["frames"]
        .as_array()
        .expect("frames")
        .clone();
    assert_eq!(frames.len(), 4, "{recording}");
    for frame in &frames {
        assert_eq!(frame["path"], frames[0]["path"], "{recording}");
        assert!(frame["recorded_at_unix_ms"].is_u64(), "{recording}");
    }
    assert_eq!(
        bench.frame_files(),
        {
            let mut names = [sha(BEFORE), sha(SETTLED)].map(|digest| format!("{digest}.png"));
            names.sort();
            names
        },
        "{recording}"
    );
    assert_eq!(
        step_of(&recording, 1)["click"]["execution"]["after_capture"]["captures"],
        4,
        "{recording}"
    );
}

#[test]
fn s12_two_presses_keep_the_after_loop_then_the_press_frame_in_order() {
    let bench = Bench::new();
    let first_after = [ANIMATION, SETTLED, SETTLED, SETTLED, SETTLED];
    let mut first_shots = vec![shot(BEFORE)];
    first_shots.extend(first_after.map(shot));
    let (_, first_log) = bench.press(&default_tap(), &first_shots);
    assert_eq!(first_log, press_log(5, 100));
    let second_after = [NEXT; 4];
    let mut second_shots = vec![shot(SETTLED_AGAIN)];
    second_shots.extend(second_after.map(shot));
    let (second, second_log) = bench.press(&default_tap(), &second_shots);
    assert_eq!(second_log, press_log(4, 100));

    let recording = bench.recording();
    // Step 2's frames in time order: the first press's after loop, then the frame the second press
    // happened on, which superseded the loop's last.
    let mut step_two = loop_frames(2, &first_after);
    if let Some(last) = step_two.last_mut() {
        last.2 = true;
    }
    step_two.push(("f0007".to_owned(), sha(SETTLED_AGAIN), false));
    assert_eq!(frames_of(&recording, 2), step_two, "{recording}");
    // Step 1's after frame is its own loop's last; the second press's capture did not rewrite it.
    let first_execution = &step_of(&recording, 1)["click"]["execution"];
    assert_eq!(first_execution["after_frame"], "f0006", "{recording}");
    assert_eq!(
        first_execution["after_capture"]["captures"], 5,
        "{recording}"
    );
    let second_execution = &step_of(&recording, 2)["click"]["execution"];
    assert_eq!(second_execution["before_frame"], "f0007", "{recording}");
    assert_eq!(second_execution["after_frame"], "f0011", "{recording}");
    assert_eq!(
        frames_of(&recording, 3),
        loop_frames(8, &second_after),
        "{recording}"
    );

    assert_eq!(second.click.step, 2);
    assert_eq!(second.before.frame_id, "f0007");
    assert!(second.before.refreshed);
    assert_eq!(
        second
            .after
            .as_ref()
            .map(|after| (after.step, after.frame_id.as_str())),
        Some((3, "f0011"))
    );
}

#[test]
fn s12_still_ms_0_takes_one_capture_untested() {
    let bench = Bench::new();
    let request = tap_request(AfterCapture {
        still_ms: Some(0),
        ..AfterCapture::for_press(None)
    });
    let (outcome, log) = bench.press(&request, &[shot(BEFORE), shot(ANIMATION)]);
    assert_eq!(log, press_log(1, 100));
    let recording = bench.recording();
    assert_eq!(
        frames_of(&recording, 2),
        loop_frames(2, &[ANIMATION]),
        "{recording}"
    );
    let execution = &step_of(&recording, 1)["click"]["execution"];
    assert_eq!(execution["after_frame"], "f0002", "{recording}");
    assert_eq!(
        execution["after_capture"],
        json!({
            "after_ms": 100,
            "interval_ms": 100,
            "still_ms": 0,
            "still_max_ms": 5000,
            "still": null,
            "waited_ms": 100,
            "captures": 1,
            "kept": 0
        }),
        "{recording}"
    );
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
}

#[test]
fn s12_a_capture_that_fails_midway_keeps_what_was_taken() {
    let bench = Bench::new();
    let shots = [shot(BEFORE), shot(ANIMATION), shot(SETTLED), FAILED];
    let (result, log) = bench.try_press(&default_tap(), &shots);
    assert_eq!(
        log,
        [
            "capture", "press", "wait 100", "capture", "wait 100", "capture", "wait 100", "capture"
        ]
    );
    // The capture's own error, loud, with the count of the captures kept.
    let error = result.expect_err("the third capture after the press fails");
    assert_eq!(error.code, LabError::device("").code, "{}", error.message);
    let details = error.details.clone().unwrap_or(Value::Null);
    assert_eq!(
        details["after"],
        json!({"after_ms": 100, "taken": true, "captures": 2}),
        "{details}"
    );
    assert_eq!(details["record"]["status"], "press_recorded", "{details}");
    assert_eq!(
        details["record"]["click"]["execution"]["after_frame"], "f0003",
        "{details}"
    );
    assert_eq!(details["record"]["after"]["frame_id"], "f0003", "{details}");
    assert_eq!(details["record"]["after"]["unfinished"], true, "{details}");

    // What was taken is kept: the two captures are frames of step 2, the last step 1's after frame.
    let recording = bench.recording();
    assert_eq!(
        frames_of(&recording, 2),
        loop_frames(2, &[ANIMATION, SETTLED]),
        "{recording}"
    );
    let execution = &step_of(&recording, 1)["click"]["execution"];
    assert_eq!(execution["effect"], "performed", "{recording}");
    assert_eq!(execution["after_frame"], "f0003", "{recording}");
    assert_eq!(
        execution["after_capture"],
        json!({
            "after_ms": 100,
            "interval_ms": 100,
            "still_ms": 300,
            "still_max_ms": 5000,
            "still": null,
            "waited_ms": 200,
            "captures": 2,
            "kept": 1,
            "unfinished": true
        }),
        "{recording}"
    );
}

#[test]
fn s12_each_parameter_out_of_its_range_is_refused_before_anything() {
    let bench = Bench::new();
    bench.capture(BEFORE);
    let before = bench.files();
    let press = AfterCapture::for_press(None);
    for (after, argument, value, min, max) in [
        (
            AfterCapture {
                interval_ms: Some(19),
                ..press
            },
            "interval_ms",
            19_u64,
            20_u64,
            1_000_u64,
        ),
        (
            AfterCapture {
                interval_ms: Some(1_001),
                ..press
            },
            "interval_ms",
            1_001,
            20,
            1_000,
        ),
        (
            AfterCapture {
                still_ms: Some(5_001),
                ..press
            },
            "still_ms",
            5_001,
            0,
            5_000,
        ),
        (
            AfterCapture {
                still_max_ms: 60_001,
                ..press
            },
            "still_max_ms",
            60_001,
            0,
            60_000,
        ),
    ] {
        let case = format!("{argument} {value}");
        let (result, log) = bench.try_press(&tap_request(after), &[]);
        assert!(log.is_empty(), "{case}: {log:?}");
        let error = result.expect_err(&case);
        assert_eq!(
            error.code,
            LabError::usage("").code,
            "{case}: {}",
            error.message
        );
        let details = error.details.clone().unwrap_or(Value::Null);
        assert_eq!(details["argument"], argument, "{case}: {details}");
        assert_eq!(details["reason"], "range", "{case}: {details}");
        assert_eq!(details["value"], value, "{case}: {details}");
        assert_eq!(details["min"], min, "{case}: {details}");
        assert_eq!(details["max"], max, "{case}: {details}");
        assert_eq!(bench.files(), before, "{case}: a file changed");
    }
}

#[test]
fn s12_more_than_120_intervals_in_the_bound_are_refused_before_anything() {
    let bench = Bench::new();
    bench.capture(BEFORE);
    let before = bench.files();
    for (still_max_ms, interval_ms) in [(12_001_u64, 100_u64), (60_000, 499), (2_401, 20)] {
        let case = format!("still_max_ms {still_max_ms}, interval_ms {interval_ms}");
        let request = tap_request(AfterCapture {
            interval_ms: Some(interval_ms),
            still_max_ms,
            ..AfterCapture::for_press(None)
        });
        let (result, log) = bench.try_press(&request, &[]);
        assert!(log.is_empty(), "{case}: {log:?}");
        let error = result.expect_err(&case);
        assert_eq!(
            error.code,
            LabError::usage("").code,
            "{case}: {}",
            error.message
        );
        // A value out of the range the interval allows: at most 120 intervals.
        let details = error.details.clone().unwrap_or(Value::Null);
        assert_eq!(details["argument"], "still_max_ms", "{case}: {details}");
        assert_eq!(details["reason"], "range", "{case}: {details}");
        assert_eq!(details["value"], still_max_ms, "{case}: {details}");
        assert_eq!(details["min"], 0, "{case}: {details}");
        assert_eq!(details["max"], 120 * interval_ms, "{case}: {details}");
        assert_eq!(details["interval_ms"], interval_ms, "{case}: {details}");
        assert_eq!(bench.files(), before, "{case}: a file changed");
    }
}
