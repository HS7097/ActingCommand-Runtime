// SPDX-License-Identifier: AGPL-3.0-only

// #463 S2-5 (contracts/lab-recording.md, "Drawing a box", "Boxes on a draft"; register §1.427):
// boxes on a recording stage one made. The presses are recorded as `tap`, `swipe` and `long-tap
// --record` record them (`record_press` under the recording lock, a fake Runtime performing the
// input, its captures taking the scripted screens and the caller's clock moving only by its
// waits); then the steps are marked with boxes as an agent marks the draft in stage two.
// - `s25_stage_two_on_a_stage_one_recording_writes_the_package_of_the_boxes`: three recorded taps
//   and a final screen; `record archive`, `record reopen`; on each step one request with a page
//   name, a recognize box and (steps 1 to 3) a press box; `record stop` writes the package: each
//   tap's click is its press box, each page's template is the crop of its recognize box, the
//   recorded taps stand as recorded; step 2's capture of the same bytes as its live frame is that
//   frame (`own`).
// - `s25_a_press_box_on_a_recorded_press_holds_its_point`: a long press, a tap and a swipe: a press
//   box on the tap (a middle step) that does not hold the point is
//   `record_click_outside_step_rect` with `recorded_point_outside_box`, one on the swipe
//   `record_click_rect_conflict`, both writing nothing; one on the long press is taken, the press
//   as recorded; a second box needs `--replace-click`.
// - `s25_a_recognize_box_takes_the_settled_capture_and_lists_the_animation`: a tap whose screen
//   after takes three captures, two of an animation and the settled screen, then the next tap: a
//   recognize box on that step adds the settled capture as a sample (`source:
//   "recording_frame"`, the same file) and lists the two others `not_added` with their scores,
//   nothing refused.
// - `s25_after_the_stop_a_press_box_is_taken_and_record_stop_regenerates`: a stopped recording's
//   point click of one pixel becomes the press box, and the next `record stop` regenerates the
//   package with it.
// Frames are synthetic 160x90 screens of flat gray with squares of gray noise; no Runtime, no
// device; the tests read no clock and never sleep.

use crate::recording::frames::{crop_png, decode_frame};
use crate::recording::store::{hex_sha256, lab_dir, old_record_path, recording_path};
use crate::{
    AfterCapture, AttachFrameRequest, CapturedFrame, ClickEffect, ClickPlan, CommitClickRequest,
    LAB_RECORD_MARK_SCHEMA, LabOcr, MarkOutcome, MarkRequest, PlanClickRequest, PlannedSwipe,
    PointPress, RecordArchiveOptions, RecordFramePorts, RecordPoint, RecordPressOutcome,
    RecordPressRequest, RecordRect, RecordReopenOptions, RecordStartOptions, RecordStopOptions,
    RecordingLock, record_archive, record_mark, record_press, record_reopen, record_start,
    record_stop,
};
use actingcommand_contract::{InputFrameReference, LabError, LabResult};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_s25_draft";
const TASK_ID: &str = "neutral_task";
const STARTED_AT: u64 = 1_700_000_000_000;
const WIDTH: u32 = 160;
const HEIGHT: u32 = 90;
const BACKGROUND: u8 = 60;

const fn rect(x: i32, y: i32, width: i32, height: i32) -> RecordRect {
    RecordRect {
        x,
        y,
        width,
        height,
    }
}

// The button of each screen, a square of noise of its own seed.
const BUTTONS: [RecordRect; 4] = [
    rect(20, 20, 16, 16),
    rect(70, 40, 16, 16),
    rect(120, 20, 16, 16),
    rect(40, 60, 16, 16),
];
const SEEDS: [u64; 4] = [0x0463_2511, 0x0463_2512, 0x0463_2513, 0x0463_2514];
// Two moments of an animation on the second screen: its button's square shows other noise.
const ANIMATION_SEEDS: [u64; 2] = [0x0463_2521, 0x0463_2522];
// The point pressed on each of the first three screens, inside its button.
const POINTS: [(i32, i32); 3] = [(28, 28), (78, 48), (128, 28)];

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

// The PNG of a flat gray screen with the square `area` of noise of `seed`, and, when `dot`, its
// bottom-right pixel white: other bytes, the same state of the screen for every box here.
fn painted(area: RecordRect, seed: u64, dot: bool) -> Vec<u8> {
    let width = WIDTH as usize;
    let mut values = vec![BACKGROUND; width * HEIGHT as usize];
    let mut noise = Noise(seed);
    for row in area.y..area.y + area.height {
        for column in area.x..area.x + area.width {
            values[row as usize * width + column as usize] = noise.between(1, 255);
        }
    }
    if dot {
        values[width * HEIGHT as usize - 1] = 255;
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

// Screen `index`: its button.
fn screen(index: usize) -> Vec<u8> {
    painted(BUTTONS[index], SEEDS[index], false)
}

// The corners of button `index`, its top-left pixel and its bottom-right one.
fn corners(index: usize) -> ((i32, i32), (i32, i32)) {
    let area = BUTTONS[index];
    (
        (area.x, area.y),
        (area.x + area.width - 1, area.y + area.height - 1),
    )
}

// A box of `usage` over button `index`.
fn button_box(usage: &str, index: usize) -> Value {
    let (from, to) = corners(index);
    boxed(usage, from, to)
}

fn boxed(usage: &str, from: (i32, i32), to: (i32, i32)) -> Value {
    json!({"use": usage, "corners": [{"x": from.0, "y": from.1}, {"x": to.0, "y": to.1}]})
}

// The mark a recognize box over button `index` becomes on step `step`.
fn box_mark(step: u32, index: usize) -> String {
    let area = BUTTONS[index];
    format!(
        "box/{step:02}/{}_{}_{}_{}",
        area.x, area.y, area.width, area.height
    )
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

// The reference of the frame the fake Runtime committed for its `number`th capture.
fn reference(number: u32) -> InputFrameReference {
    InputFrameReference {
        frame_id: serde_json::from_value(json!(format!("frame_{number:032x}")))
            .expect("a frame id"),
        width: WIDTH,
        height: HEIGHT,
    }
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
            input_frame: Some(reference(self.taken)),
        })
    }
}

// The screen after a press as one capture 800 ms after it, without a stillness test.
fn one_capture_after() -> AfterCapture {
    AfterCapture {
        still_ms: Some(0),
        ..AfterCapture::for_press(Some(800))
    }
}

// `tap <x> <y> --record`, the screen after it taken as `after` says.
fn tap_at(point: (i32, i32), after: AfterCapture) -> RecordPressRequest {
    RecordPressRequest {
        click: PlanClickRequest::default(),
        point_press: Some(PointPress::Tap {
            point: RecordPoint {
                x: point.0,
                y: point.1,
            },
        }),
        after,
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

// The entries of the package `record stop` wrote.
fn package(lab: &Value) -> BTreeMap<String, Vec<u8>> {
    let bytes = fs::read(lab["path"].as_str().expect("package path")).expect("package");
    let container = if lab["container"] == "zip" {
        ContentContainer::Zip
    } else {
        ContentContainer::Json
    };
    expand_content_container(&bytes, container, ContainmentLimits::default())
        .expect("package container")
}

// The task of `entries`.
fn task_of(entries: &BTreeMap<String, Vec<u8>>) -> Value {
    serde_json::from_slice(
        entries
            .get(&format!("resources/operations/{TASK_ID}/task.json"))
            .expect("task.json"),
    )
    .expect("task JSON")
}

// The crop of `area` of screen `png`, as the Lab cuts a template.
fn crop_of(png: Vec<u8>, area: RecordRect) -> Vec<u8> {
    crop_png(&decode_frame(png, "the screen").expect("frame"), area).expect("crop")
}

// The click a press box over button `index` gives the package.
fn rect_click(index: usize) -> Value {
    let area = BUTTONS[index];
    json!({"kind": "rect", "x": area.x, "y": area.y, "width": area.width, "height": area.height})
}

// A state directory with an active session of `INSTANCE` (the header `record start` writes) and its
// Lab recording, before any step, without an OCR model. The lock is held as one command holds it
// and dropped before the directory.
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
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_463_s25", "Lab-CLI")
            .expect("recording lock");
        let bench = Self {
            lock,
            ocr: LabOcr::from_vision_root(&temp.path().join("no-vision")),
            temp,
        };
        bench.header("active");
        record_start(
            &bench.lock,
            &RecordStartOptions {
                game: Some("neutral".to_owned()),
                server: Some("test".to_owned()),
                locale: Some("en-US".to_owned()),
                ..RecordStartOptions::default()
            },
        )
        .expect("record start");
        bench
    }

    fn state(&self) -> PathBuf {
        self.temp.path().join("state")
    }

    // The session header with `status`, as the `record` command writes it.
    fn header(&self, status: &str) {
        fs::write(
            old_record_path(&self.state(), INSTANCE),
            serde_json::to_vec(&json!({
                "record_id": RECORD_ID,
                "task_id": TASK_ID,
                "instance": INSTANCE,
                "status": status,
                "started_at_unix_ms": STARTED_AT
            }))
            .expect("session JSON"),
        )
        .expect("session file");
    }

    // `record_press` of `request`, the fake Runtime performing the input and its captures taking
    // `shots` in order; the press must be recorded.
    fn press(&self, request: &RecordPressRequest, shots: Vec<Vec<u8>>) -> RecordPressOutcome {
        let mut device = Screens {
            shots: shots.into(),
            taken: 0,
            now: Duration::ZERO,
        };
        let mut send = |_: &ClickPlan, _: InputFrameReference| -> LabResult<CommitClickRequest> {
            Ok(receipt())
        };
        record_press(&self.lock, request, &mut send, &mut device).unwrap_or_else(|error| {
            panic!(
                "the press: {}: {} {:?}",
                error.code, error.message, error.details
            )
        })
    }

    fn mark(&self, value: Value) -> Result<MarkOutcome, LabError> {
        record_mark(&self.lock, &request(value), false, &self.ocr)
    }

    // `record mark` of `value`, which must stand.
    fn marked(&self, value: Value) -> MarkOutcome {
        self.mark(value).unwrap_or_else(|error| {
            panic!(
                "record mark: {}: {} {:?}",
                error.code, error.message, error.details
            )
        })
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

    fn recording(&self) -> Value {
        serde_json::from_slice(
            &fs::read(recording_path(&self.state(), RECORD_ID)).expect("step table"),
        )
        .expect("step table JSON")
    }

    fn files(&self) -> Vec<(String, Vec<u8>)> {
        files_under(&lab_dir(&self.state(), RECORD_ID))
    }
}

fn step_of(recording: &Value, step: u32) -> &Value {
    recording["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .find(|item| item["index"] == step)
        .unwrap_or_else(|| panic!("step {step}: {recording}"))
}

// The work order's test of the whole of stage two on a stage-one recording: three recorded taps and
// a final screen, each press's screen after one capture and the capture before the next press its
// step's live frame; `record archive` and `record reopen`; on each step one request with a page
// name, a recognize box and a press box (the final step a recognize box alone); `record stop`
// writes the package, in which each tap's click is its press box and each page's template the crop
// of its recognize box from the step's frame. The taps stand as recorded.
#[test]
fn s25_stage_two_on_a_stage_one_recording_writes_the_package_of_the_boxes() {
    let bench = Bench::new();
    for (index, point) in POINTS.into_iter().enumerate() {
        bench.press(
            &tap_at(point, one_capture_after()),
            vec![screen(index), screen(index + 1)],
        );
    }
    let recorded = bench.recording();
    // Step 2 holds the capture after the first tap (f0002), superseded by the capture before the
    // second (f0003), of the same bytes.
    let step_2 = step_of(&recorded, 2);
    assert_eq!(step_2["frames"][0]["frame_id"], "f0002", "{recorded}");
    assert_eq!(step_2["frames"][0]["superseded"], true, "{recorded}");
    assert_eq!(step_2["frames"][1]["frame_id"], "f0003", "{recorded}");

    record_archive(
        &bench.lock,
        &RecordArchiveOptions {
            record_id: None,
            copy_to: None,
            written_by: "gate_463_s25".to_owned(),
        },
    )
    .unwrap_or_else(|error| panic!("record archive: {}: {:?}", error.code, error.details));
    bench.header("archived");
    let reopened = record_reopen(&bench.lock, &RecordReopenOptions::default())
        .unwrap_or_else(|error| panic!("record reopen: {}: {:?}", error.code, error.details));
    assert_eq!(reopened.status, "reopened");
    bench.header("active");

    let pages = ["first", "second", "third", "done"];
    for (index, page) in pages.into_iter().enumerate() {
        let step = u32::try_from(index + 1).expect("step");
        let mut boxes = vec![button_box("recognize", index)];
        if index < 3 {
            boxes.push(button_box("press", index));
        }
        let marked = bench.marked(json!({"step": step, "page": page, "boxes": boxes}));
        assert_eq!(marked.marks[0].id, box_mark(step, index), "step {step}");
        let views = serde_json::to_value(&marked.boxes).expect("boxes JSON");
        let frames = views[0]["frames"].as_array().expect("frames");
        assert_eq!(frames[0]["taken"], "own", "step {step}: {views}");
        assert!(score(&frames[0]) >= 0.95, "step {step}: {views}");
        if index < 3 {
            let click = marked.click.as_ref().expect("the click");
            assert_eq!(click.rect, BUTTONS[index], "step {step}");
            assert_eq!(click.source, "point", "step {step}");
        }
        if step == 2 {
            // The capture after the first tap is the live frame's bytes: that frame, not added.
            assert_eq!(frames[1]["taken"], "own", "{views}");
            assert_eq!(frames[1]["superseded"], "f0002", "{views}");
            assert_eq!(frames[1]["frame_id"], "f0003", "{views}");
            assert!(marked.samples.is_empty(), "{:?}", marked.samples);
        }
    }
    let marked = bench.recording();
    for (index, point) in POINTS.into_iter().enumerate() {
        let step = u32::try_from(index + 1).expect("step");
        let before = &step_of(&recorded, step)["click"];
        let after = &step_of(&marked, step)["click"];
        assert_eq!(after["execution"], before["execution"], "step {step}");
        assert_eq!(
            after["execution"]["point"],
            json!({"x": point.0, "y": point.1})
        );
    }

    let generated = bench.stop();
    assert_eq!(generated["lab_status"], "generated", "{generated}");
    let entries = package(&generated);
    let task = task_of(&entries);
    let operations = task["operations"].as_array().expect("operations");
    assert_eq!(operations.len(), 3, "{task}");
    for (index, operation) in operations.iter().enumerate() {
        assert_eq!(operation["click"], rect_click(index), "{task}");
    }
    for (index, area) in BUTTONS.into_iter().enumerate() {
        let step = u32::try_from(index + 1).expect("step");
        let asset = format!(
            "resources/operations/{TASK_ID}/assets/box_{step:02}_{}_{}_{}_{}.png",
            area.x, area.y, area.width, area.height
        );
        assert_eq!(
            entries.get(&asset),
            Some(&crop_of(screen(index), area)),
            "{asset}"
        );
    }
}

// The work order's test of a press box on a recorded press: a long press (step 1), a tap (step 2,
// a middle step) and a swipe (step 3), then the final screen. A press box on the tap that does not
// hold its point is refused with the word that says so, the point and the box named; one on the
// swipe is refused as a press box beside a drag; neither writes anything. One on the long press
// holds its point and is taken: the click's rectangle is the box, its source, gesture and
// execution as recorded. A second box on it needs `--replace-click`, as a second click does.
#[test]
fn s25_a_press_box_on_a_recorded_press_holds_its_point() {
    let bench = Bench::new();
    bench.press(
        &RecordPressRequest {
            click: PlanClickRequest::default(),
            point_press: Some(PointPress::LongPress {
                point: RecordPoint { x: 28, y: 28 },
                duration_ms: 800,
            }),
            after: one_capture_after(),
        },
        vec![screen(0), screen(1)],
    );
    bench.press(
        &tap_at(POINTS[1], one_capture_after()),
        vec![screen(1), screen(2)],
    );
    bench.press(
        &RecordPressRequest {
            click: PlanClickRequest {
                swipe: Some(PlannedSwipe {
                    from: RecordPoint { x: 128, y: 28 },
                    to: RecordPoint { x: 60, y: 28 },
                    duration_ms: 300,
                }),
                ..PlanClickRequest::default()
            },
            point_press: None,
            after: one_capture_after(),
        },
        vec![screen(2), screen(3)],
    );
    let before = bench.files();
    let recorded = bench.recording();

    let error = bench
        .mark(json!({"step": 2, "boxes": [boxed("press", (100, 10), (115, 25))]}))
        .expect_err("a press box that does not hold the tap's point");
    assert_eq!(
        error.code, "record_click_outside_step_rect",
        "{}",
        error.message
    );
    let details = error.details.expect("details");
    assert_eq!(details["reason"], "recorded_point_outside_box", "{details}");
    assert_eq!(details["step"], 2, "{details}");
    assert_eq!(details["point"], json!({"x": 78, "y": 48}), "{details}");
    assert_eq!(details["box"], json!(rect(100, 10, 16, 16)), "{details}");
    assert_eq!(bench.files(), before, "a file changed");

    let error = bench
        .mark(json!({"step": 3, "boxes": [button_box("press", 2)]}))
        .expect_err("a press box on a swipe");
    assert_eq!(
        error.code, "record_click_rect_conflict",
        "{}",
        error.message
    );
    assert_eq!(bench.files(), before, "a file changed");

    let taken = bench.marked(json!({"step": 1, "boxes": [button_box("press", 0)]}));
    let click = taken.click.as_ref().expect("the click");
    assert_eq!(click.rect, BUTTONS[0]);
    let stored = bench.recording();
    let was = &step_of(&recorded, 1)["click"];
    let now = &step_of(&stored, 1)["click"];
    assert_eq!(now["rect"], json!(BUTTONS[0]), "{now}");
    assert_eq!(now["source"], "point", "{now}");
    assert_eq!(
        now["gesture"],
        json!({"kind": "long_press", "duration_ms": 800}),
        "{now}"
    );
    assert_eq!(now["execution"], was["execution"], "{now}");

    let error = bench
        .mark(json!({"step": 1, "boxes": [boxed("press", (24, 24), (31, 31))]}))
        .expect_err("a second box without --replace-click");
    assert_eq!(error.code, "record_click_exists", "{}", error.message);
    let replaced = bench.marked(json!({
        "step": 1,
        "boxes": [boxed("press", (24, 24), (31, 31))],
        "replace_click": true
    }));
    assert_eq!(
        replaced.click.as_ref().map(|click| click.rect),
        Some(rect(24, 24, 8, 8))
    );
}

// The work order's test of the base images on a draft: the tap on the first screen takes its
// screen after in three captures (one every 100 ms, the bound 200 ms): two moments of an animation
// of the second screen's button, then the second screen settled; the second tap's capture before it
// is that screen with one pixel off elsewhere, the step's live frame. A recognize box over the
// button on that step: the settled capture is added as a sample, the stored frame itself (`source:
// "recording_frame"`, the same file and sha256, a frame id of its own); the two moments of the
// animation are listed `not_added`, below the threshold, with their scores; nothing is refused.
#[test]
fn s25_a_recognize_box_takes_the_settled_capture_and_lists_the_animation() {
    let bench = Bench::new();
    let settled = screen(1);
    let animation = ANIMATION_SEEDS.map(|seed| painted(BUTTONS[1], seed, false));
    bench.press(
        &tap_at(
            POINTS[0],
            AfterCapture {
                after_ms: 100,
                interval_ms: Some(100),
                still_ms: None,
                still_max_ms: 200,
            },
        ),
        vec![
            screen(0),
            animation[0].clone(),
            animation[1].clone(),
            settled.clone(),
        ],
    );
    bench.press(
        &tap_at(POINTS[1], one_capture_after()),
        vec![painted(BUTTONS[1], SEEDS[1], true), screen(2)],
    );
    let recorded = bench.recording();
    let step_2 = step_of(&recorded, 2);
    let ids = step_2["frames"]
        .as_array()
        .expect("frames")
        .iter()
        .map(|frame| (frame["frame_id"].clone(), frame["superseded"].clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        [
            (json!("f0002"), json!(true)),
            (json!("f0003"), json!(true)),
            (json!("f0004"), json!(true)),
            (json!("f0005"), json!(false))
        ],
        "{recorded}"
    );
    assert_eq!(
        step_of(&recorded, 1)["click"]["execution"]["after_frame"],
        "f0004"
    );

    let marked = bench.marked(json!({"step": 2, "page": "second",
                                     "boxes": [button_box("recognize", 1)]}));
    let views = serde_json::to_value(&marked.boxes).expect("boxes JSON");
    let frames = views[0]["frames"]
        .as_array()
        .expect("frames")
        .iter()
        .filter(|frame| frame["taken"] != "elsewhere")
        .cloned()
        .collect::<Vec<_>>();
    let taken = frames
        .iter()
        .map(|frame| frame["taken"].clone())
        .collect::<Vec<_>>();
    assert_eq!(taken, ["own", "not_added", "not_added", "added"], "{views}");
    assert_eq!(frames[0]["frame_id"], "f0005", "{views}");
    let because = json!({"reason": "below_threshold", "mark": box_mark(2, 1), "box": 0});
    for (frame, capture) in frames[1..3].iter().zip(["f0002", "f0003"]) {
        assert_eq!(frame["superseded"], capture, "{views}");
        assert!(frame.get("frame_id").is_none(), "{views}");
        assert!(score(frame) < 0.95, "{views}");
        assert_eq!(frame["because"], because, "{views}");
    }
    assert_eq!(frames[3]["superseded"], "f0004", "{views}");
    assert_eq!(frames[3]["frame_id"], "f0007", "{views}");
    assert!(score(&frames[3]) >= 0.95, "{views}");
    assert!(frames[3].get("because").is_none(), "{views}");

    assert_eq!(marked.samples.len(), 1, "{:?}", marked.samples);
    assert_eq!(marked.marks[0].self_test.frames, 2);
    let stored = bench.recording();
    let frames = step_of(&stored, 2)["frames"].as_array().expect("frames");
    let sample = frames.last().expect("the sample");
    let capture = &frames[2];
    assert_eq!(sample["frame_id"], "f0007", "{stored}");
    assert_eq!(sample["role"], "sample", "{stored}");
    assert_eq!(sample["source"], "recording_frame", "{stored}");
    assert_eq!(sample["superseded"], false, "{stored}");
    assert_eq!(sample["sha256"], hex_sha256(&settled), "{stored}");
    assert_eq!(sample["path"], capture["path"], "{stored}");
}

// The work order's test after the stop: a stopped recording whose tap was recorded by its point
// generated a click of one pixel; a press box on that tap is taken (the answer says the recording
// is stopped), and the next `record stop` regenerates the package with the box as the click.
#[test]
fn s25_after_the_stop_a_press_box_is_taken_and_record_stop_regenerates() {
    let bench = Bench::new();
    bench.press(
        &tap_at(POINTS[0], one_capture_after()),
        vec![screen(0), screen(1)],
    );
    bench.marked(json!({"step": 1, "boxes": [button_box("recognize", 0)]}));
    bench.marked(json!({"step": 2, "boxes": [button_box("recognize", 1)]}));
    let generated = bench.stop();
    assert_eq!(generated["lab_status"], "generated", "{generated}");
    assert_eq!(
        task_of(&package(&generated))["operations"][0]["click"],
        json!({"kind": "rect", "x": 28, "y": 28, "width": 1, "height": 1})
    );
    bench.header("stopped");

    let taken = bench.marked(json!({"step": 1, "boxes": [button_box("press", 0)]}));
    assert!(taken.after_stop);
    assert_eq!(
        taken.click.as_ref().map(|click| click.rect),
        Some(BUTTONS[0])
    );
    let regenerated = bench.stop();
    assert_eq!(regenerated["lab_status"], "regenerated", "{regenerated}");
    assert_eq!(
        task_of(&package(&regenerated))["operations"][0]["click"],
        rect_click(0)
    );
}
