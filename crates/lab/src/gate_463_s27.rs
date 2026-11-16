// SPDX-License-Identifier: AGPL-3.0-only

// #463 S2-7 (contracts/lab-recording.md, "Transitions", "Steps, frames and remediation"; register
// §1.440): a page transition named by frames the recording stored, and a press before the last that
// changed nothing dropped. The presses are recorded as `tap --record` records them (`record_press`
// under the recording lock, a fake Runtime performing the input, its captures taking the scripted
// screens and the caller's clock moving only by its waits); then the draft is worked offline.
// - `s27_a_transition_from_a_stored_frame_writes_the_package_a_file_writes`: a loading screen stage
//   one stored between two presses, named by its frame id, is the transition's frame (the stored
//   file itself, `source: "recording_frame"`, no new file); `record stop` writes the package the
//   same image given as a file writes, but for the generation time.
// - `s27_a_frame_id_the_recording_does_not_hold_is_refused`: `record_frame_unknown`, as the frame or
//   as a sample; nothing is written.
// - `s27_a_stored_frame_whose_file_no_longer_holds_its_bytes_is_refused`:
//   `record_frame_hash_mismatch`; nothing is written.
// - `s27_a_middle_press_that_changed_nothing_is_dropped_renumbered_and_restored`: a press whose
//   screen after is its screen before, and the next step's screen the same, is dropped; undoing the
//   drop (`restore_step`) gives the recording back as it was; dropped again, `record stop` numbers
//   the package's steps 1 to 3 over the recorded steps 1, 3 and 4.
// - `s27_a_middle_press_whose_screen_after_differs_is_refused`: `record_drop_unproven`,
//   `screen_changed`; nothing is written.
// - `s27_a_middle_press_after_which_the_next_step_starts_elsewhere_is_refused`:
//   `record_drop_unproven`, `next_screen_differs`; nothing is written.
// The drop of the last step is pinned, unchanged, by gate_449_rec_arch's
// `s15_fix2_dropped_and_converted_steps_are_listed_and_not_in_the_material`.
// Frames are synthetic 160x90 screens of flat gray with squares of gray noise; no Runtime, no
// device; the tests read no clock and never sleep.

use crate::recording::store::{hex_sha256, lab_dir, old_record_path, recording_path};
use crate::{
    AfterCapture, AttachFrameRequest, CapturedFrame, ClickEffect, ClickPlan, CommitClickRequest,
    LAB_RECORD_MARK_SCHEMA, LabCode, LabOcr, MarkOutcome, MarkRequest, PlanClickRequest,
    PointPress, RecordFramePorts, RecordPoint, RecordPressRequest, RecordRect, RecordStartOptions,
    RecordStopOptions, RecordingLock, record_mark, record_press, record_start, record_stop,
};
use actingcommand_contract::{InputFrameReference, LabError, LabResult};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use actingcommand_recognition::color_digest::FRAME_CHANGE_MAX_MILLI;
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_s27_frames";
const TASK_ID: &str = "neutral_task";
const STARTED_AT: u64 = 1_700_000_000_000;
const WIDTH: u32 = 160;
const HEIGHT: u32 = 90;

const fn rect(x: i32, y: i32, width: i32, height: i32) -> RecordRect {
    RecordRect {
        x,
        y,
        width,
        height,
    }
}

// The button of each screen, a square of noise of its own seed.
const BUTTONS: [RecordRect; 3] = [
    rect(20, 20, 16, 16),
    rect(70, 40, 16, 16),
    rect(120, 20, 16, 16),
];
const SEEDS: [u64; 3] = [0x0463_2711, 0x0463_2712, 0x0463_2713];
// The gray of each screen: two screens differ as a whole, far beyond the frame-change bound (a
// square of noise alone moves the measure of a 160x90 frame by less than the bound).
const BACKGROUNDS: [u8; 3] = [60, 140, 200];
// The point pressed on each of the first two screens, inside its button.
const POINTS: [(i32, i32); 2] = [(28, 28), (78, 48)];
// A point of the second screen outside its button: a press there changes nothing.
const IDLE_POINT: (i32, i32) = (150, 80);
// The loading screen between the first and the second screen: its own gray and its own square of
// noise, where no screen has a button.
const LOADING: RecordRect = rect(100, 60, 16, 16);
const LOADING_SEED: u64 = 0x0463_2721;
const LOADING_BACKGROUND: u8 = 100;

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

// The PNG of a flat screen of gray `background` with the square `area` of noise of `seed`.
fn painted(background: u8, area: RecordRect, seed: u64) -> Vec<u8> {
    let width = WIDTH as usize;
    let mut values = vec![background; width * HEIGHT as usize];
    let mut noise = Noise(seed);
    for row in area.y..area.y + area.height {
        for column in area.x..area.x + area.width {
            values[row as usize * width + column as usize] = noise.between(1, 255);
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

// Screen `index`: its gray and its button.
fn screen(index: usize) -> Vec<u8> {
    painted(BACKGROUNDS[index], BUTTONS[index], SEEDS[index])
}

// The loading screen.
fn loading() -> Vec<u8> {
    painted(LOADING_BACKGROUND, LOADING, LOADING_SEED)
}

fn request(mut value: Value) -> MarkRequest {
    value["schema_version"] = json!(LAB_RECORD_MARK_SCHEMA);
    serde_json::from_value(value).expect("a record mark request")
}

// The template mark `id` over `area`.
fn template(id: &str, area: RecordRect) -> Value {
    json!({"id": id, "family": "template", "region": area})
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

// The screen after a press as two captures, 100 ms and 200 ms after it, the bound reached at the
// second.
fn two_captures_after() -> AfterCapture {
    AfterCapture {
        after_ms: 100,
        interval_ms: Some(100),
        still_ms: None,
        still_max_ms: 100,
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

// The files of a package, the task's provenance without its generation time (the time the
// recording was last written, which every `record mark` changes).
fn without_generation_time(mut entries: BTreeMap<String, Vec<u8>>) -> BTreeMap<String, Vec<u8>> {
    for (name, bytes) in &mut entries {
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

// The step table without the time it was last written, which every write of it changes.
fn without_update_time(mut recording: Value) -> Value {
    recording
        .as_object_mut()
        .expect("step table")
        .remove("updated_at_unix_ms")
        .expect("the update time");
    recording
}

// Every file but the step table, by name and bytes.
fn beside_the_step_table(files: Vec<(String, Vec<u8>)>) -> Vec<(String, Vec<u8>)> {
    files
        .into_iter()
        .filter(|(name, _)| name != "recording.json")
        .collect()
}

// The names of the stored frames among `files`.
fn frame_files(files: &[(String, Vec<u8>)]) -> Vec<String> {
    files
        .iter()
        .filter(|(name, _)| name.starts_with("frames/"))
        .map(|(name, _)| name.clone())
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

// The stored frame of step `step` whose bytes are `png`.
fn frame_of<'r>(recording: &'r Value, step: u32, png: &[u8]) -> &'r Value {
    let sha256 = hex_sha256(png);
    step_of(recording, step)["frames"]
        .as_array()
        .expect("frames")
        .iter()
        .find(|frame| frame["sha256"] == sha256.as_str())
        .unwrap_or_else(|| panic!("no frame {sha256} in step {step}: {recording}"))
}

// The serial number and the package number of each step of a package, as its provenance names
// them.
fn numbered_steps(task: &Value) -> Vec<(Value, Value)> {
    task["provenance"]["steps"]
        .as_array()
        .unwrap_or_else(|| panic!("no step provenance in {task}"))
        .iter()
        .map(|step| (step["step"].clone(), step["record_index"].clone()))
        .collect()
}

// The `drop_problem` of a `record_drop_unproven` refusal, its outcome carrying the same word, and
// its details.
fn unproven(error: LabError) -> (String, Value) {
    assert_eq!(
        error.code,
        LabCode::RecordDropUnproven.as_str(),
        "{}: {:?}",
        error.message,
        error.details
    );
    let details = error.details.expect("details");
    assert_eq!(
        details["outcome"]["values"]["drop_problem"], details["drop_problem"],
        "{details}"
    );
    let problem = details["drop_problem"]
        .as_str()
        .unwrap_or_else(|| panic!("no drop_problem in {details}"))
        .to_owned();
    (problem, details)
}

// The frame-change measure's value a refusal names.
fn change_of(details: &Value) -> u64 {
    details["frame_change_milli"]
        .as_u64()
        .unwrap_or_else(|| panic!("no frame_change_milli in {details}"))
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
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_463_s27", "Lab-CLI")
            .expect("recording lock");
        let bench = Self {
            lock,
            ocr: LabOcr::from_vision_root(&temp.path().join("no-vision")),
            temp,
        };
        bench.header();
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

    // A file of the test's own, outside the state directory.
    fn scratch(&self, name: &str) -> PathBuf {
        self.temp.path().join(name)
    }

    // The active session header, as the `record` command writes it.
    fn header(&self) {
        fs::write(
            old_record_path(&self.state(), INSTANCE),
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
    }

    // `record_press` of `request`, the fake Runtime performing the input and its captures taking
    // `shots` in order; the press must be recorded.
    fn press(&self, request: &RecordPressRequest, shots: Vec<Vec<u8>>) {
        let mut device = Screens {
            shots: shots.into(),
            taken: 0,
            now: Duration::ZERO,
        };
        let mut send = |_: &ClickPlan, _: InputFrameReference| -> LabResult<CommitClickRequest> {
            Ok(receipt())
        };
        if let Err(error) = record_press(&self.lock, request, &mut send, &mut device) {
            panic!(
                "the press: {}: {} {:?}",
                error.code, error.message, error.details
            );
        }
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

    // Each `(step, page, button)`: the step's page with a template mark over the button.
    fn pages(&self, pages: &[(u32, &str, usize)]) {
        for (step, page, button) in pages {
            self.marked(json!({
                "step": step,
                "page": page,
                "add": [template(&format!("ui/{page}"), BUTTONS[*button])]
            }));
        }
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

// Two presses whose first took its screen after in two captures: the loading screen, then the
// second screen; the second press led to the final screen. Steps 1 to 3 each carry a page with a
// template mark over its button. The loading capture is a superseded frame of step 2.
fn loading_recording() -> Bench {
    let bench = Bench::new();
    bench.press(
        &tap_at(POINTS[0], two_captures_after()),
        vec![screen(0), loading(), screen(1)],
    );
    bench.press(
        &tap_at(POINTS[1], one_capture_after()),
        vec![screen(1), screen(2)],
    );
    bench.pages(&[(1, "first", 0), (2, "second", 1), (3, "done", 2)]);
    bench
}

// The page transition of step 1 over the loading screen, its frame given by `frame` (a file's
// `frame` or a stored frame's `frame_id`).
fn loading_transition(frame: Value) -> Value {
    let mut transition = json!({"kind": "page", "add": [template("load/square", LOADING)]});
    if let (Some(object), Some(given)) = (transition.as_object_mut(), frame.as_object()) {
        for (name, value) in given {
            object.insert(name.clone(), value.clone());
        }
    }
    json!({"step": 1, "transition": transition})
}

// The work order's first test: the loading screen stage one stored between the two presses, named
// by its frame id, becomes the transition's frame: the stored file itself (its path and sha256),
// with a frame id of its own and `source: "recording_frame"`, and no file is added to the frame
// store. The same image given as a PNG file to the same recording made the same way gives the same
// package, but for the generation time.
#[test]
fn s27_a_transition_from_a_stored_frame_writes_the_package_a_file_writes() {
    let by_file = loading_recording();
    let png = by_file.scratch("loading.png");
    fs::write(&png, loading()).expect("the loading screen's file");
    by_file.marked(loading_transition(
        json!({"frame": png.display().to_string()}),
    ));

    let by_id = loading_recording();
    let recorded = by_id.recording();
    let stored = frame_of(&recorded, 2, &loading()).clone();
    assert_eq!(stored["superseded"], true, "{recorded}");
    let frames_before = frame_files(&by_id.files());
    let marked = by_id.marked(loading_transition(json!({"frame_id": stored["frame_id"]})));
    assert_eq!(marked.step, Some(1));
    assert_eq!(marked.marks.len(), 1, "{:?}", marked.marks);
    assert_eq!(
        frame_files(&by_id.files()),
        frames_before,
        "a frame file was added"
    );
    let taken = by_id.recording();
    let frame = &step_of(&taken, 1)["transition"]["frames"][0];
    assert_eq!(frame["role"], "transition", "{taken}");
    assert_eq!(frame["source"], "recording_frame", "{taken}");
    assert_eq!(frame["sha256"], stored["sha256"], "{taken}");
    assert_eq!(frame["path"], stored["path"], "{taken}");
    assert_ne!(frame["frame_id"], stored["frame_id"], "{taken}");
    assert_eq!(
        frame_of(&taken, 2, &loading()),
        &stored,
        "the stored frame stays as it was"
    );

    let from_file = by_file.stop();
    let from_id = by_id.stop();
    assert_eq!(from_file["lab_status"], "generated", "{from_file}");
    assert_eq!(from_id["lab_status"], "generated", "{from_id}");
    let entries = package(&from_id);
    assert_eq!(
        task_of(&entries)["operations"][0]["transition"]["kind"],
        "page",
        "{}",
        task_of(&entries)
    );
    assert_eq!(
        without_generation_time(entries),
        without_generation_time(package(&from_file))
    );
}

// The work order's second test: a frame id the recording does not hold, as the transition's frame
// or as one of its samples, is `record_frame_unknown` naming it, and nothing is written.
#[test]
fn s27_a_frame_id_the_recording_does_not_hold_is_refused() {
    let bench = loading_recording();
    let recorded = bench.recording();
    let stored = frame_of(&recorded, 2, &loading())["frame_id"].clone();
    let before = bench.files();
    for given in [
        json!({"frame_id": "f0999"}),
        json!({"frame_id": stored, "sample_frame_ids": ["f0999"]}),
    ] {
        let error = bench
            .mark(loading_transition(given))
            .expect_err("a frame the recording does not hold");
        assert_eq!(
            error.code,
            LabCode::RecordFrameUnknown.as_str(),
            "{}",
            error.message
        );
        let details = error.details.expect("details");
        assert_eq!(details["frame_id"], "f0999", "{details}");
        assert_eq!(details["step"], 1, "{details}");
        assert_eq!(bench.files(), before, "a file changed");
    }
}

// The work order's third test: the stored file of the loading capture no longer holds the bytes its
// sha256 names: naming that frame is `record_frame_hash_mismatch`, and nothing is written.
#[test]
fn s27_a_stored_frame_whose_file_no_longer_holds_its_bytes_is_refused() {
    let bench = loading_recording();
    let recorded = bench.recording();
    let stored = frame_of(&recorded, 2, &loading()).clone();
    fs::write(
        stored["path"].as_str().expect("the stored frame's path"),
        screen(2),
    )
    .expect("other bytes in the stored frame's file");
    let before = bench.files();
    let error = bench
        .mark(loading_transition(json!({"frame_id": stored["frame_id"]})))
        .expect_err("a stored frame whose bytes changed");
    assert_eq!(
        error.code, "record_frame_hash_mismatch",
        "{}",
        error.message
    );
    assert_eq!(bench.files(), before, "a file changed");
}

// The work order's fourth test: three presses, the second on the second screen where it changes
// nothing (its screen before, its screen after and the third press's screen before are the second
// screen), and the final screen. Steps 1, 3 and 4 carry pages. `drop_step` of step 2, a step before
// the last, is taken: the step stays in the file, dropped, its frames as recorded; `restore_step`
// gives the recording back as it was before the drop (every file, and the step table but for the
// time it was written). Dropped again, `record stop` writes a package of three steps, numbered 1 to
// 3 over the recorded steps 1, 3 and 4.
#[test]
fn s27_a_middle_press_that_changed_nothing_is_dropped_renumbered_and_restored() {
    let bench = Bench::new();
    bench.press(
        &tap_at(POINTS[0], one_capture_after()),
        vec![screen(0), screen(1)],
    );
    bench.press(
        &tap_at(IDLE_POINT, one_capture_after()),
        vec![screen(1), screen(1)],
    );
    bench.press(
        &tap_at(POINTS[1], one_capture_after()),
        vec![screen(1), screen(2)],
    );
    bench.pages(&[(1, "first", 0), (3, "second", 1), (4, "done", 2)]);
    let kept = bench.recording();
    let kept_files = beside_the_step_table(bench.files());
    let execution = &step_of(&kept, 2)["click"]["execution"];
    let screen_one = hex_sha256(&screen(1));
    for member in ["before_frame", "after_frame"] {
        let frame_id = execution[member].as_str().expect("a frame id");
        let named = kept["steps"]
            .as_array()
            .expect("steps")
            .iter()
            .flat_map(|step| step["frames"].as_array().expect("frames"))
            .find(|frame| frame["frame_id"] == frame_id)
            .unwrap_or_else(|| panic!("{member} {frame_id}: {kept}"));
        assert_eq!(named["sha256"], screen_one.as_str(), "{member}: {kept}");
    }

    let dropped = bench.marked(json!({"step_action": {"kind": "drop_step", "step": 2}}));
    assert_eq!(dropped.status, "step_dropped");
    assert_eq!(dropped.step, Some(2));
    let stored = bench.recording();
    assert_eq!(step_of(&stored, 2)["dropped"], true, "{stored}");
    assert_eq!(
        step_of(&stored, 2)["frames"],
        step_of(&kept, 2)["frames"],
        "{stored}"
    );

    let restored = bench.marked(json!({"step_action": {"kind": "restore_step", "step": 2}}));
    assert_eq!(restored.status, "step_restored");
    assert_eq!(restored.step, Some(2));
    assert_eq!(
        without_update_time(bench.recording()),
        without_update_time(kept)
    );
    assert_eq!(beside_the_step_table(bench.files()), kept_files);

    bench.marked(json!({"step_action": {"kind": "drop_step", "step": 2}}));
    let generated = bench.stop();
    assert_eq!(generated["lab_status"], "generated", "{generated}");
    let task = task_of(&package(&generated));
    assert_eq!(
        task["operations"].as_array().map(Vec::len),
        Some(2),
        "{task}"
    );
    assert_eq!(
        numbered_steps(&task),
        [
            (json!(1), json!(1)),
            (json!(2), json!(3)),
            (json!(3), json!(4))
        ],
        "{task}"
    );
}

// The work order's fifth test: a press that changed the screen (its screen after is the next
// screen): `drop_step` of it, a step before the last, is `record_drop_unproven` with
// `screen_changed`, the two frames and the measure's value above the bound, and nothing is written.
#[test]
fn s27_a_middle_press_whose_screen_after_differs_is_refused() {
    let bench = Bench::new();
    bench.press(
        &tap_at(POINTS[0], one_capture_after()),
        vec![screen(0), screen(1)],
    );
    bench.press(
        &tap_at(POINTS[1], one_capture_after()),
        vec![screen(1), screen(2)],
    );
    let recorded = bench.recording();
    let execution = step_of(&recorded, 1)["click"]["execution"].clone();
    let before = bench.files();
    let (problem, details) = unproven(
        bench
            .mark(json!({"step_action": {"kind": "drop_step", "step": 1}}))
            .expect_err("a press that changed the screen"),
    );
    assert_eq!(problem, "screen_changed", "{details}");
    assert_eq!(details["step"], 1, "{details}");
    assert_eq!(
        details["before_frame"], execution["before_frame"],
        "{details}"
    );
    assert_eq!(
        details["after_frame"], execution["after_frame"],
        "{details}"
    );
    assert!(
        change_of(&details) > u64::from(FRAME_CHANGE_MAX_MILLI),
        "{details}"
    );
    assert_eq!(bench.files(), before, "a file changed");
}

// The work order's sixth test: a press whose screen after is its screen before (the first screen
// still), after which the screen changed by itself: the next press was sent on the second screen.
// `drop_step` of the first step is `record_drop_unproven` with `next_screen_differs`, naming the
// next step and the frame its press was sent on, and nothing is written.
#[test]
fn s27_a_middle_press_after_which_the_next_step_starts_elsewhere_is_refused() {
    let bench = Bench::new();
    bench.press(
        &tap_at(POINTS[0], one_capture_after()),
        vec![screen(0), screen(0)],
    );
    bench.press(
        &tap_at(POINTS[1], one_capture_after()),
        vec![screen(1), screen(2)],
    );
    let recorded = bench.recording();
    let next_press = step_of(&recorded, 2)["click"]["execution"].clone();
    assert_eq!(
        frame_of(&recorded, 2, &screen(1))["frame_id"],
        next_press["before_frame"],
        "{recorded}"
    );
    let before = bench.files();
    let (problem, details) = unproven(
        bench
            .mark(json!({"step_action": {"kind": "drop_step", "step": 1}}))
            .expect_err("a press after which the next step starts on another screen"),
    );
    assert_eq!(problem, "next_screen_differs", "{details}");
    assert_eq!(details["next_step"], 2, "{details}");
    assert_eq!(
        details["next_before_frame"], next_press["before_frame"],
        "{details}"
    );
    assert!(
        change_of(&details) > u64::from(FRAME_CHANGE_MAX_MILLI),
        "{details}"
    );
    assert_eq!(bench.files(), before, "a file changed");
}
