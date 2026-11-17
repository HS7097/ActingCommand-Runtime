// SPDX-License-Identifier: AGPL-3.0-only

// #463 KR-1 (contracts/lab-recording.md, "Key steps"; contracts/linear-steps.md, "Key steps";
// register §1.449): a recording's step may press the back key in place of a click, declared after
// the fact and never sent while recording. A recording is made as stage one makes it (presses
// through `record_press` with a fake Runtime, its captures taking the scripted screens and the
// caller's clock moving only by its waits; three presses give four steps), then marked with colour
// marks. Frames are synthetic 160x90 screens of gray with squares of one colour; no Runtime, no
// device, no OCR model; the tests read no clock and never sleep.
// - `kr1_an_executed_press_replaced_by_the_key_generates_the_key_step` (1): the key replaces step
//   2's executed press with `replace_click`; `recording.json` holds the key with that click whole,
//   and `record stop` writes `step_02_key` (its page, the next page, the key, the named guard, the
//   arrival, a click's post-input wait, no click, no retry, no input recheck), admitted by the one
//   admission.
// - `kr1_taking_the_key_back_restores_the_recording_and_the_package` (2): `key: false` gives the
//   recording back byte for byte but for `updated_at_unix_ms`; on the stopped recording the package
//   generated again after a key taken back is the one generated before it, but for its generation
//   time.
// - `kr1_a_key_step_without_a_guard_is_listed_and_refused_until_one_is_named` (3).
// - `kr1_each_refusal_of_a_key_writes_nothing` (4).
// - `kr1_an_optional_step_inserted_after_stop_is_closed_by_the_key` (5).
// - `kr1_a_key_on_the_last_step_with_an_effect_arrives_at_the_target_page` (6).
// - `kr1_a_key_on_the_first_step_is_the_offline_first_decision` (7).
// - `kr1_record_review_answers_the_key_and_the_press_it_replaced_without_drawing_a_press` (8).
// - `kr1_a_key_step_has_its_regression_case_on_the_frame_of_the_press_it_replaced` (9).
// - `kr1_a_word_outside_the_vocabulary_is_refused_before_anything_is_read` (10).

use crate::codes::{KeyProblem, LabCode, OptionalAfterProblem};
use crate::recording::crosscheck::admit as admit_entries;
use crate::recording::frames::decode_frame;
use crate::recording::generate::DEFAULT_ARRIVAL_TIMEOUT_MS;
use crate::recording::model::LabRecording;
use crate::recording::preview::PRESS_COLOUR;
use crate::recording::store::{old_record_path, recording_path};
use crate::{
    AfterCapture, AttachFrameRequest, CapturedFrame, ClickEffect, ClickPlan, CommitClickRequest,
    LAB_RECORD_MARK_SCHEMA, LabOcr, MarkRequest, PlanClickRequest, PointPress, RecordFramePorts,
    RecordPoint, RecordPressRequest, RecordRect, RecordReviewRequest, RecordStartOptions,
    RecordStopOptions, RecordingLock, RegressionVerdict, record_mark, record_press, record_ready,
    record_review, record_start, record_stop, run_regression_case,
};
use actingcommand_contract::{InputFrameReference, LabError, LabResult};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0463_kr1";
const TASK_ID: &str = "neutral_task";
/// The package's id, `<game>.<server>.<task_id>`.
const PACKAGE_ID: &str = "neutral.test.neutral_task";
const STARTED_AT: u64 = 1_700_000_000_000;
const WIDTH: u32 = 160;
const HEIGHT: u32 = 90;
const GRAY: [u8; 3] = [60, 60, 60];
const RED: [u8; 3] = [230, 20, 20];
const GREEN: [u8; 3] = [20, 230, 20];
const BLUE: [u8; 3] = [20, 20, 230];
const YELLOW: [u8; 3] = [230, 230, 20];
const CYAN: [u8; 3] = [20, 230, 230];
/// The back key, the vocabulary's one word.
const BACK: &str = "back";
/// Test 5's pop-up mark.
const POPUP_MARK: &str = "popup_mark";

const fn rect(x: i32, y: i32, width: i32, height: i32) -> RecordRect {
    RecordRect {
        x,
        y,
        width,
        height,
    }
}

/// Each screen's page square, a colour mark's region.
const SQUARE: RecordRect = rect(0, 0, 16, 16);
/// Test 5's pop-up square, gray on every recorded screen.
const SECOND: RecordRect = rect(32, 0, 16, 16);
/// A click rectangle of test 4's refusals.
const CLICK_AREA: RecordRect = rect(100, 40, 16, 16);
/// The point each press presses, on no square.
const POINT: RecordPoint = RecordPoint { x: 130, y: 70 };

// Whether `area` covers the pixel at `(x, y)`.
fn covers(area: RecordRect, x: u32, y: u32) -> bool {
    let (column, row) = (i64::from(x), i64::from(y));
    column >= i64::from(area.x)
        && row >= i64::from(area.y)
        && column < i64::from(area.x) + i64::from(area.width)
        && row < i64::from(area.y) + i64::from(area.height)
}

// The PNG of a gray screen with each of `squares` in its colour.
fn screen(squares: &[(RecordRect, [u8; 3])]) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let color = squares
                .iter()
                .find(|(area, _)| covers(*area, x, y))
                .map_or(GRAY, |(_, color)| *color);
            pixels.extend(color);
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

// Four screens, each with its page square in its own colour.
fn four_screens() -> [Vec<u8>; 4] {
    [RED, GREEN, BLUE, YELLOW].map(|color| screen(&[(SQUARE, color)]))
}

// The colour mark `id` over `area`.
fn color(id: &str, area: RecordRect) -> Value {
    json!({"id": id, "family": "color", "region": area})
}

fn request(mut value: Value) -> MarkRequest {
    value["schema_version"] = json!(LAB_RECORD_MARK_SCHEMA);
    serde_json::from_value(value).expect("a record mark request")
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
    fn new() -> Self {
        let temp = TempDir::new().expect("temporary directory");
        let state = temp.path().join("state");
        fs::create_dir_all(&state).expect("state directory");
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_463_kr1", "Lab-CLI")
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

    // The recording of `screens`: one press on each screen but the last, the screen after it one
    // capture later; then each step marked with the colour mark `square_<step>` on its square.
    fn marked_recording(screens: &[Vec<u8>]) -> Self {
        let bench = Self::new();
        for pair in screens.windows(2) {
            bench.press(vec![pair[0].clone(), pair[1].clone()]);
        }
        let steps = u32::try_from(screens.len()).expect("a step count");
        for step in 1..=steps {
            bench.marked(json!({
                "step": step,
                "add": [color(&format!("square_{step}"), SQUARE)]
            }));
        }
        bench
    }

    fn state(&self) -> PathBuf {
        self.temp.path().join("state")
    }

    // The session header, as the `record` command writes it with `status`.
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

    // A press at `POINT` recorded by the command that makes it, the fake Runtime performing the
    // input and its captures taking `shots`: the screen before the press, then the screen after it
    // one capture 800 ms later.
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

    // `record mark` of `value`, its answer as JSON.
    fn mark(&self, value: Value) -> Result<Value, LabError> {
        record_mark(&self.lock, &request(value), false, &self.ocr)
            .map(|outcome| serde_json::to_value(&outcome).expect("mark JSON"))
    }

    // `record mark` of `value`, which must stand.
    fn marked(&self, value: Value) -> Value {
        self.mark(value).unwrap_or_else(|error| {
            panic!(
                "record mark: {}: {} {:?}",
                error.code, error.message, error.details
            )
        })
    }

    // `record mark` of `value`, which must be refused with nothing of the recording written.
    fn refused(&self, value: Value) -> LabError {
        let before = self.recording_bytes();
        let error = self
            .mark(value.clone())
            .expect_err("the request is refused");
        assert_eq!(
            self.recording_bytes(),
            before,
            "nothing written for {value}: {}",
            error.message
        );
        error
    }

    // `record stop`: the `lab` output with its status.
    fn stop(&self) -> Result<Value, LabError> {
        record_stop(&self.lock, &RecordStopOptions::default(), &self.ocr).map(|outcome| {
            let mut lab = serde_json::to_value(&outcome.lab).expect("lab JSON");
            lab["lab_status"] = json!(outcome.lab_status);
            lab
        })
    }

    // `record stop`, which must stand.
    fn stopped(&self) -> Value {
        self.stop().unwrap_or_else(|error| {
            panic!(
                "record stop: {}: {} {:?}",
                error.code, error.message, error.details
            )
        })
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
        .unwrap_or_else(|error| panic!("record ready: {}: {}", error.code, error.message));
        serde_json::to_value(&outcome).expect("record ready JSON")
    }

    // The bytes of `recording.json`.
    fn recording_bytes(&self) -> Vec<u8> {
        fs::read(recording_path(&self.state(), RECORD_ID)).expect("recording.json")
    }

    // `recording.json` as JSON.
    fn recording(&self) -> Value {
        serde_json::from_slice(&self.recording_bytes()).expect("recording JSON")
    }

    // The package `record stop` wrote (`lab`), expanded into a checkout's `packs/<package_id>/`.
    fn package(&self, lab: &Value) -> PathBuf {
        let root = self.temp.path().join("repo").join("packs").join(PACKAGE_ID);
        for (name, bytes) in entries(lab) {
            let file = name
                .split('/')
                .fold(root.clone(), |path, part| path.join(part));
            fs::create_dir_all(file.parent().expect("a parent")).expect("package directories");
            fs::write(file, bytes).expect("package file");
        }
        root
    }
}

// The entries of the package `record stop` wrote (`lab`).
fn entries(lab: &Value) -> BTreeMap<String, Vec<u8>> {
    let bytes = fs::read(lab["path"].as_str().expect("package path")).expect("package");
    let container = if lab["container"] == "zip" {
        ContentContainer::Zip
    } else {
        ContentContainer::Json
    };
    expand_content_container(&bytes, container, ContainmentLimits::default())
        .expect("package container")
}

// The task of the package's `entries`.
fn task(entries: &BTreeMap<String, Vec<u8>>) -> Value {
    entries
        .iter()
        .find(|(name, _)| name.ends_with("/task.json"))
        .map(|(_, bytes)| serde_json::from_slice::<Value>(bytes).expect("task JSON"))
        .expect("the package's task.json")
}

// The operation `id` of `task`.
fn operation<'t>(task: &'t Value, id: &str) -> &'t Value {
    task["operations"]
        .as_array()
        .expect("operations")
        .iter()
        .find(|operation| operation["id"] == id)
        .unwrap_or_else(|| panic!("no operation {id} in {task}"))
}

// The ids of `task`'s operations, in order.
fn operation_ids(task: &Value) -> Vec<String> {
    task["operations"]
        .as_array()
        .expect("operations")
        .iter()
        .map(|operation| operation["id"].as_str().expect("an id").to_owned())
        .collect()
}

// The member names of `value`, an object.
fn members(value: &Value) -> BTreeSet<String> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("no object: {value}"))
        .keys()
        .cloned()
        .collect()
}

fn names(list: &[&str]) -> BTreeSet<String> {
    list.iter().map(|name| (*name).to_owned()).collect()
}

// The refusal's details.
fn details(error: &LabError) -> &Value {
    error
        .details
        .as_ref()
        .unwrap_or_else(|| panic!("no details: {}: {}", error.code, error.message))
}

// The `key_problem` of a `record_key_invalid` refusal, the same in its details and its outcome.
fn key_problem(error: &LabError) -> String {
    assert_eq!(
        error.code,
        LabCode::RecordKeyInvalid.as_str(),
        "{}: {:?}",
        error.message,
        error.details
    );
    let details = details(error);
    assert_eq!(
        details["outcome"]["values"]["key_problem"], details["key_problem"],
        "{details}"
    );
    details["key_problem"]
        .as_str()
        .unwrap_or_else(|| panic!("no key_problem in {details}"))
        .to_owned()
}

// The key declared on step `step` in place of its click.
fn key_on(step: u32) -> Value {
    json!({"step": step, "key": {"key": BACK}, "replace_click": true})
}

// The colours of the pixels of the PNG file at `path`.
fn colours(path: &Value) -> BTreeSet<[u8; 3]> {
    let png = fs::read(path.as_str().expect("a path")).expect("the PNG file");
    let frame = decode_frame(png, "a PNG of the test").expect("a PNG").frame;
    let channels = match frame.pixel_format {
        PixelFormat::Rgb8 => 3,
        PixelFormat::Rgba8 => 4,
    };
    frame
        .pixels
        .chunks(channels)
        .map(|pixel| [pixel[0], pixel[1], pixel[2]])
        .collect()
}

// The work order's test 1: step 2's press, executed, is replaced by the key with `replace_click`.
// `recording.json` holds the key with the click it replaced, whole, and no click; the step's frames
// stand. `record stop` writes the key step's operation as "Key steps" declares it: `step_02_key`
// from its page to the next, the key, the guard named, the arrival at the next page, a click's
// post-input wait, and nothing else (no click, no retry, no input recheck); the package is
// admitted by the one admission, the source parser and the kernel's rules.
#[test]
fn kr1_an_executed_press_replaced_by_the_key_generates_the_key_step() {
    let bench = Bench::marked_recording(&four_screens());
    let before = bench.recording();
    let press = before["steps"][1]["click"].clone();
    assert_eq!(press["execution"]["effect"], "performed", "{press}");
    let frames = before["steps"][1]["frames"].clone();

    let mut declared = key_on(2);
    declared["click_guard"] = json!("square_2");
    let answer = bench.marked(declared);
    assert_eq!(answer["step"], 2, "{answer}");
    assert_eq!(answer["click"], Value::Null, "{answer}");
    assert_eq!(answer["step_state"]["effect"], "key_declared", "{answer}");
    assert_eq!(answer["key"]["key"], BACK, "{answer}");
    assert_eq!(answer["key"]["replaced"], press, "{answer}");

    let after = bench.recording();
    let step = &after["steps"][1];
    assert_eq!(step["index"], 2, "{step}");
    assert_eq!(step["click"], Value::Null, "{step}");
    assert_eq!(step["click_guard"], "square_2", "{step}");
    assert_eq!(
        members(&step["key"]),
        names(&["declared_at_unix_ms", "key", "replaced"]),
        "{step}"
    );
    assert_eq!(step["key"]["key"], BACK, "{step}");
    assert_eq!(step["key"]["replaced"], press, "{step}");
    assert_eq!(step["frames"], frames, "the step's frames stand: {step}");

    let lab = bench.stopped();
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    assert_eq!(
        lab["key_steps"],
        json!([{"step": 2, "op": "step_02_key", "key": BACK, "guard": "square_2",
                "replaced_click": true}]),
        "{lab}"
    );
    let written = entries(&lab);
    let task = task(&written);
    assert_eq!(
        operation_ids(&task),
        ["step_01_click", "step_02_key", "step_03_click"],
        "{task}"
    );
    let key_step = operation(&task, "step_02_key");
    assert_eq!(
        members(key_step),
        names(&[
            "expect_after",
            "from",
            "guard",
            "id",
            "key",
            "post_delay_ms",
            "provenance",
            "purpose",
            "to"
        ]),
        "{key_step}"
    );
    assert_eq!(key_step["from"], "step_02", "{key_step}");
    assert_eq!(key_step["to"], "step_03", "{key_step}");
    assert_eq!(key_step["key"], json!({"key": BACK}), "{key_step}");
    assert_eq!(
        key_step["guard"],
        json!({"page_id": "step_02", "target_id": "square_2", "expected_rect": SQUARE,
               "color_probe": "square_2"}),
        "{key_step}"
    );
    assert_eq!(
        key_step["expect_after"],
        json!({"page_id": "step_03", "timeout_ms": DEFAULT_ARRIVAL_TIMEOUT_MS,
               "interval_ms": 500}),
        "{key_step}"
    );
    assert_eq!(
        key_step["post_delay_ms"],
        operation(&task, "step_01_click")["post_delay_ms"],
        "{key_step}"
    );
    assert_eq!(
        key_step["provenance"]["key"],
        json!({"key": BACK, "replaced_click": true}),
        "{key_step}"
    );
    if let Err(error) = admit_entries(&written, &bench.ocr) {
        panic!(
            "the package is not admitted: {}: {} {:?}",
            error.code, error.message, error.details
        );
    }
}

// The work order's test 2: `key: false` takes the key back and gives back the click it replaced
// exactly: `recording.json` is byte for byte what it was before the key was declared, but for
// `updated_at_unix_ms`, which every write sets. On the stopped recording (marking after stop) a key
// declared and taken back again leaves the package `record stop` generates again the one generated
// before it, but for its generation time (the task's `provenance.generated_at_unix_ms`, the
// recording's last update).
#[test]
fn kr1_taking_the_key_back_restores_the_recording_and_the_package() {
    let bench = Bench::marked_recording(&four_screens());
    let before = bench.recording_bytes();
    let declared = bench.marked(key_on(2));
    assert_eq!(
        declared["step_state"]["effect"], "key_declared",
        "{declared}"
    );
    assert_ne!(bench.recording_bytes(), before, "the key is written");
    let taken_back = bench.marked(json!({"step": 2, "key": false}));
    assert_eq!(
        taken_back["step_state"]["effect"], "click_executed",
        "{taken_back}"
    );
    assert_eq!(taken_back["key"], Value::Null, "{taken_back}");
    let earlier: LabRecording = serde_json::from_slice(&before).expect("the earlier recording");
    let mut restored: LabRecording =
        serde_json::from_slice(&bench.recording_bytes()).expect("the restored recording");
    assert!(
        restored.updated_at_unix_ms >= earlier.updated_at_unix_ms,
        "updated_at_unix_ms is set by every write"
    );
    restored.updated_at_unix_ms = earlier.updated_at_unix_ms;
    let mut bytes = serde_json::to_vec_pretty(&restored).expect("recording bytes");
    bytes.push(b'\n');
    assert_eq!(
        String::from_utf8(bytes).expect("UTF-8"),
        String::from_utf8(before).expect("UTF-8"),
        "byte for byte but for updated_at_unix_ms"
    );
    // On a step without a key, key false changes nothing but the time of the write.
    let unchanged = bench.marked(json!({"step": 3, "key": false}));
    assert_eq!(
        unchanged["step_state"]["effect"], "click_executed",
        "{unchanged}"
    );

    let generated = bench.stopped();
    assert_eq!(generated["lab_status"], "generated", "{generated}");
    assert_eq!(generated["key_steps"], Value::Null, "{generated}");
    let first = entries(&generated);
    bench.header("stopped");
    let stopped_declared = bench.marked(key_on(2));
    assert_eq!(stopped_declared["after_stop"], true, "{stopped_declared}");
    bench.marked(json!({"step": 2, "key": false}));
    let regenerated = bench.stopped();
    assert_eq!(regenerated["lab_status"], "regenerated", "{regenerated}");
    let second = entries(&regenerated);
    assert_eq!(
        first.keys().collect::<Vec<_>>(),
        second.keys().collect::<Vec<_>>()
    );
    for (name, bytes) in &first {
        if name.ends_with("/task.json") {
            continue;
        }
        assert_eq!(Some(bytes), second.get(name), "{name}");
    }
    let mut tasks = [task(&first), task(&second)];
    for each in &mut tasks {
        each["provenance"]
            .as_object_mut()
            .expect("provenance")
            .remove("generated_at_unix_ms")
            .expect("its generation time");
    }
    assert_eq!(tasks[0], tasks[1], "but for its generation time");
}

// The work order's test 3: a key step that names no guard is listed by `record ready` with its own
// word, `key_guard_missing` (the code `record stop` answers, `record_key_invalid`, and the hint that
// settles it), the one item; `record stop` refuses it (`guard_missing`) and writes nothing; the mark
// that declared the key was not refused. With the guard named, both pass.
#[test]
fn kr1_a_key_step_without_a_guard_is_listed_and_refused_until_one_is_named() {
    let bench = Bench::marked_recording(&four_screens());
    let press_frame = bench.recording()["steps"][1]["click"]["execution"]["before_frame"].clone();
    assert!(press_frame.is_string(), "{press_frame}");
    bench.marked(key_on(2));

    let ready = bench.ready();
    assert_eq!(ready["status"], "draft", "{ready}");
    assert_eq!(
        ready["missing"],
        json!([{"order": 1, "step": 2, "artifact_step": 2, "frame_id": press_frame,
                "what": "key_guard_missing", "code": "record_key_invalid",
                "hint": "record mark --step 2 --click-guard <id>"}]),
        "{ready}"
    );
    assert_eq!(ready["missing_total"], 1, "{ready}");
    assert_eq!(ready["checks"], json!([]), "{ready}");
    // The key step's press is no point any more: only steps 1 and 3 press one.
    let point_steps = ready["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .map(|warning| (warning["what"].clone(), warning["step"].clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        point_steps,
        [
            (json!("press_point_only"), json!(1)),
            (json!("press_point_only"), json!(3))
        ],
        "{ready}"
    );

    let before = bench.recording_bytes();
    let refusal = bench.stop().expect_err("a key step without a guard");
    assert_eq!(key_problem(&refusal), KeyProblem::GuardMissing.as_str());
    let details = details(&refusal);
    assert_eq!(details["step"], 2, "{details}");
    assert_eq!(details["artifact_step"], 2, "{details}");
    assert_eq!(details["missing_total"], 1, "{details}");
    assert_eq!(bench.recording_bytes(), before, "nothing written");

    bench.marked(json!({"step": 2, "click_guard": "square_2"}));
    let ready = bench.ready();
    assert_eq!(ready["status"], "ready", "{ready}");
    assert_eq!(ready["missing"], json!([]), "{ready}");
    assert_eq!(ready["checks_after"], json!([]), "{ready}");
    let lab = bench.stopped();
    assert_eq!(lab["lab_status"], "generated", "{lab}");
}

// The work order's test 4: each refusal of a key, one word per cause, nothing written:
// `word_unknown`; beside the key a click (a rectangle, a source mark, a press box: `with_click`),
// an application operation (`with_application`), a retry (`with_retry`), a choice (`holds_choice`);
// a step that holds a choice (`holds_choice`), a step whose effect is an application operation
// (`application_step`), the application entry step (`application_entry`), a dropped step
// (`step_not_effective`); a click without `replace_click`, the refusal of a second effect
// (`record_step_effect_exists`, effect `click`), and a click on a step whose effect is the key
// (effect `key`).
#[test]
fn kr1_each_refusal_of_a_key_writes_nothing() {
    let bench = Bench::marked_recording(&four_screens());
    let beside = |member: &str, value: Value| {
        let mut asked = key_on(2);
        asked[member] = value;
        asked
    };
    let press_box =
        json!([{"use": "press", "corners": [{"x": 100, "y": 40}, {"x": 115, "y": 55}]}]);
    for (asked, word) in [
        (
            json!({"step": 2, "key": {"key": "home"}, "replace_click": true}),
            KeyProblem::WordUnknown,
        ),
        (
            beside("click", json!({"region": CLICK_AREA})),
            KeyProblem::WithClick,
        ),
        (
            beside("click", json!({"from": "square_2"})),
            KeyProblem::WithClick,
        ),
        (beside("boxes", press_box), KeyProblem::WithClick),
        (
            beside("application", json!({"action": "restart"})),
            KeyProblem::WithApplication,
        ),
        (
            beside("retry", json!({"max_attempts": 2, "interval_ms": 1000})),
            KeyProblem::WithRetry,
        ),
        (beside("choice", json!(true)), KeyProblem::HoldsChoice),
    ] {
        let error = bench.refused(asked.clone());
        assert_eq!(key_problem(&error), word.as_str(), "{asked}");
        assert_eq!(details(&error)["step"], 2, "{asked}");
    }
    assert_eq!(
        details(&bench.refused(json!({"step": 2, "key": {"key": "home"}})))["key"],
        "home"
    );

    // A second effect without replace_click is the refusal every second effect gets.
    let second = bench.refused(json!({"step": 2, "key": {"key": BACK}}));
    assert_eq!(
        second.code, "record_step_effect_exists",
        "{}",
        second.message
    );
    assert_eq!(details(&second)["effect"], "click", "{:?}", second.details);

    // A step that holds a choice.
    bench.marked(json!({"step": 2, "choice": true}));
    assert_eq!(
        key_problem(&bench.refused(key_on(2))),
        KeyProblem::HoldsChoice.as_str()
    );
    bench.marked(json!({"step": 2, "choice": false}));

    // A step whose effect is an application operation: the last step, declared.
    bench.marked(json!({"step": 4, "application": {"action": "restart"}}));
    assert_eq!(
        key_problem(&bench.refused(key_on(4))),
        KeyProblem::ApplicationStep.as_str()
    );

    // Once the key is the step's effect, a click lands on it only after the key is taken back.
    bench.marked(key_on(2));
    let click = bench.refused(json!({"step": 2, "click": {"region": CLICK_AREA},
                                     "replace_click": true}));
    assert_eq!(click.code, "record_step_effect_exists", "{}", click.message);
    assert_eq!(details(&click)["effect"], "key", "{:?}", click.details);
    let choice = bench.refused(json!({"step": 2, "choice": true}));
    assert_eq!(key_problem(&choice), KeyProblem::HoldsChoice.as_str());

    // A dropped step.
    let dropping = Bench::marked_recording(&four_screens());
    dropping.marked(json!({"step_action": {"kind": "drop_step", "step": 4}}));
    let dropped = dropping.refused(key_on(4));
    assert_eq!(key_problem(&dropped), KeyProblem::StepNotEffective.as_str());
    assert_eq!(details(&dropped)["step"], 4);

    // The application entry step, which has no frame.
    let entry = Bench::new();
    entry.marked(json!({"application": {"action": "launch"}}));
    let entry_key = entry.refused(json!({"step": 1, "key": {"key": BACK}}));
    assert_eq!(
        key_problem(&entry_key),
        KeyProblem::ApplicationEntry.as_str()
    );
}

// The work order's test 5: a pop-up met after step 1's press, inserted after the stop and closed by
// the key: `optional_after.key` in place of `click`. It is generated as an optional key step,
// `step_02_key` with `optional` and `key`, once its guard is named (until then `record ready`
// lists it, `key_guard_missing`); an insertion with both a click and a key is `record_key_invalid`
// `with_click`, one with neither `record_optional_after_invalid` `no_effect`; neither writes
// anything.
#[test]
fn kr1_an_optional_step_inserted_after_stop_is_closed_by_the_key() {
    let bench = Bench::marked_recording(&four_screens());
    assert_eq!(bench.stopped()["lab_status"], "generated");
    bench.header("stopped");
    let popup = bench.temp.path().join("popup.png");
    fs::write(&popup, screen(&[(SECOND, CYAN)])).expect("the pop-up's frame");
    let insertion = |effect: Value| {
        let mut inserted = json!({
            "step": 1,
            "frame": popup.display().to_string(),
            "page": "popup",
            "add": [color(POPUP_MARK, SECOND)]
        });
        if let (Some(object), Some(effect)) = (inserted.as_object_mut(), effect.as_object()) {
            for (name, value) in effect {
                object.insert(name.clone(), value.clone());
            }
        }
        json!({"optional_after": inserted})
    };

    let both = bench.refused(insertion(json!({
        "click": {"region": CLICK_AREA}, "key": {"key": BACK}
    })));
    assert_eq!(key_problem(&both), KeyProblem::WithClick.as_str());
    let neither = bench.refused(insertion(json!({})));
    assert_eq!(
        neither.code,
        LabCode::RecordOptionalAfterInvalid.as_str(),
        "{}",
        neither.message
    );
    assert_eq!(
        details(&neither)["optional_after_problem"],
        OptionalAfterProblem::NoEffect.as_str()
    );

    let inserted = bench.marked(insertion(json!({"key": {"key": BACK}})));
    assert_eq!(inserted["status"], "optional_step_inserted", "{inserted}");
    assert_eq!(inserted["step"], 5, "{inserted}");
    assert_eq!(
        inserted["step_state"]["effect"], "key_declared",
        "{inserted}"
    );
    assert_eq!(inserted["key"]["key"], BACK, "{inserted}");
    assert_eq!(inserted["key"]["replaced"], Value::Null, "{inserted}");

    let ready = bench.ready();
    let lacking = ready["missing"]
        .as_array()
        .expect("missing")
        .iter()
        .map(|item| (item["what"].clone(), item["step"].clone()))
        .collect::<Vec<_>>();
    assert_eq!(lacking, [(json!("key_guard_missing"), json!(5))], "{ready}");

    bench.marked(json!({"step": 5, "click_guard": POPUP_MARK}));
    let lab = bench.stopped();
    assert_eq!(lab["lab_status"], "regenerated", "{lab}");
    let task = task(&entries(&lab));
    assert_eq!(
        operation_ids(&task),
        [
            "step_01_click",
            "step_02_key",
            "step_03_click",
            "step_04_click"
        ],
        "{task}"
    );
    let key_step = operation(&task, "step_02_key");
    assert_eq!(key_step["from"], "step_02_popup", "{key_step}");
    assert_eq!(key_step["to"], "step_03", "{key_step}");
    assert_eq!(key_step["key"], json!({"key": BACK}), "{key_step}");
    assert_eq!(
        key_step["optional"],
        json!({"settle_ms": 2000}),
        "{key_step}"
    );
    assert_eq!(key_step["guard"]["target_id"], POPUP_MARK, "{key_step}");
    assert_eq!(key_step["click"], Value::Null, "{key_step}");
    assert_eq!(
        key_step["provenance"]["key"],
        json!({"key": BACK, "replaced_click": false}),
        "{key_step}"
    );
}

// The work order's test 6: the last step with an effect presses the key: its `expect_after` is the
// task's target page, the last step's.
#[test]
fn kr1_a_key_on_the_last_step_with_an_effect_arrives_at_the_target_page() {
    let bench = Bench::marked_recording(&four_screens());
    let mut declared = key_on(3);
    declared["click_guard"] = json!("square_3");
    bench.marked(declared);
    let task = task(&entries(&bench.stopped()));
    assert_eq!(task["target_page"], "step_04", "{task}");
    let key_step = operation(&task, "step_03_key");
    assert_eq!(key_step["to"], task["target_page"], "{key_step}");
    assert_eq!(
        key_step["expect_after"]["page_id"], task["target_page"],
        "{key_step}"
    );
}

// The work order's test 7: the first step with an effect presses the key: the offline first decision
// `record stop` answers is the press of the key, `would_press_key` of `step_01_key` with the key
// `back`, on step 1's primary frame.
#[test]
fn kr1_a_key_on_the_first_step_is_the_offline_first_decision() {
    let bench = Bench::marked_recording(&four_screens());
    let primary = bench.recording()["steps"][0]["frames"]
        .as_array()
        .expect("frames")
        .iter()
        .rev()
        .find(|frame| frame["role"] == "primary" && frame["superseded"] == false)
        .map(|frame| frame["frame_id"].clone())
        .expect("step 1's primary frame");
    let mut declared = key_on(1);
    declared["click_guard"] = json!("square_1");
    bench.marked(declared);
    let lab = bench.stopped();
    let decision = &lab["first_decision"];
    assert_eq!(decision["status"], "would_press_key", "{lab}");
    assert_eq!(decision["operation_label"], "step_01_key", "{decision}");
    assert_eq!(decision["key"], BACK, "{decision}");
    assert_eq!(decision["frame"], primary, "{decision}");
}

// The work order's test 8: `record review` of a key step answers its effect, the key, and the press
// it replaced (the executed point press stage one recorded), its screen before and after those of
// that press, and its preview draws no press: no pixel of the press's colour, which step 1's
// preview, a press, shows.
#[test]
fn kr1_record_review_answers_the_key_and_the_press_it_replaced_without_drawing_a_press() {
    let bench = Bench::marked_recording(&four_screens());
    let execution = bench.recording()["steps"][1]["click"]["execution"].clone();
    bench.marked(key_on(2));
    let review = |step: u32| {
        let answer = record_review(
            &bench.state(),
            INSTANCE,
            &RecordReviewRequest {
                record_id: None,
                step: Some(step),
                no_preview: false,
            },
            &RecordStopOptions::default(),
            &bench.ocr,
        )
        .unwrap_or_else(|error| panic!("record review: {}: {}", error.code, error.message));
        let answer = serde_json::to_value(&answer).expect("review JSON");
        answer["steps"][0].clone()
    };
    let key_step = review(2);
    assert_eq!(key_step["step"], 2, "{key_step}");
    assert_eq!(key_step["kind"], "key", "{key_step}");
    assert_eq!(
        key_step["press"],
        json!({"key": BACK,
               "replaced": {"point": POINT, "rect": rect(POINT.x, POINT.y, 1, 1),
                            "source": "point", "outcome": "executed"},
               "source": "declared", "outcome": "declared"}),
        "{key_step}"
    );
    assert_eq!(
        key_step["before"]["frame_id"], execution["before_frame"],
        "{key_step}"
    );
    assert_eq!(
        key_step["after"]["frame_id"], execution["after_frame"],
        "{key_step}"
    );
    let drawn = colours(&key_step["before"]["preview"]);
    assert!(!drawn.contains(&PRESS_COLOUR), "no press drawn: {drawn:?}");
    let pressed = review(1);
    assert_eq!(pressed["kind"], "tap", "{pressed}");
    assert!(
        colours(&pressed["before"]["preview"]).contains(&PRESS_COLOUR),
        "a press is drawn on step 1's preview"
    );
}

// The work order's test 9: a key step is an effective step: its regression case is written, on the
// frame the press its key replaced was sent on (step 2's green screen), its source the recording
// and the step, and it passes when read from the disk and run on the written package.
#[test]
fn kr1_a_key_step_has_its_regression_case_on_the_frame_of_the_press_it_replaced() {
    let bench = Bench::marked_recording(&four_screens());
    let mut declared = key_on(2);
    declared["click_guard"] = json!("square_2");
    bench.marked(declared);
    let lab = bench.stopped();
    let cases = &lab["cases"];
    assert_eq!(
        cases["counts"],
        json!({"steps": 3, "final": 1, "endings": 0}),
        "{lab}"
    );
    assert_eq!(
        cases["ids"],
        json!(["step_01", "step_02", "step_03", "final"]),
        "{lab}"
    );
    let directory = PathBuf::from(cases["directory"].as_str().expect("the cases' directory"));
    let case_json = fs::read(directory.join("step_02").join("case.json")).expect("case.json");
    let scene = fs::read(directory.join("step_02").join("scene.png")).expect("scene.png");
    let case: Value = serde_json::from_slice(&case_json).expect("case JSON");
    assert_eq!(
        case["source"],
        json!({"record_id": RECORD_ID, "step": 2}),
        "{case}"
    );
    assert_eq!(
        case["expect"],
        json!({"observe": {"page": "neutral/step_02"},
               "recognize": [{"target": "square_2", "passed": true}]}),
        "{case}"
    );
    let frame = decode_frame(scene.clone(), "the case's scene")
        .expect("the scene")
        .frame;
    assert_eq!(frame.pixels[..3], GREEN, "the frame of the replaced press");
    let package = bench.package(&lab);
    let verdict = run_regression_case(&package, &case_json, &scene, &bench.ocr);
    assert_eq!(verdict.verdict, RegressionVerdict::Passed, "{verdict:?}");
}

// The work order's test 10: a key word outside the vocabulary is refused at the request, before the
// recording is read or written: on a step the recording does not hold it is still `word_unknown`,
// not `record_step_not_found`, and the recording's files are as they were.
#[test]
fn kr1_a_word_outside_the_vocabulary_is_refused_before_anything_is_read() {
    let bench = Bench::marked_recording(&four_screens());
    for word in ["home", "Back", "4", ""] {
        let error = bench.refused(json!({"step": 99, "key": {"key": word}}));
        assert_eq!(
            key_problem(&error),
            KeyProblem::WordUnknown.as_str(),
            "{word}"
        );
        assert_eq!(details(&error)["key"], word, "{word}");
        assert_eq!(details(&error)["step"], 99, "{word}");
    }
    let not_a_key = bench
        .mark(json!({"step": 2, "key": true}))
        .expect_err("true is no key");
    assert_eq!(not_a_key.code, "validation_failed", "{}", not_a_key.message);
}
