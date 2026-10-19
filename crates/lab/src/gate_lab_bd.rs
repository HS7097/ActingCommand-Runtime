// SPDX-License-Identifier: AGPL-3.0-only

// #411 Lab BD (exec key 109; register §1.133): a Lab recording declares a single-touch drag with
// a vertical brake. `record mark --click x,y,w,h --brake-corner x,y,w,h` (the request's
// `"click": {"region", "brake_corner"}`) declares it with the Runtime's fixed timings; both boxes
// lie inside the frame and the corner at least 100 px below the top, and it takes no duration,
// destination, long press, source mark or followed template. `recording.json` keeps it as the
// click's gesture `single_touch_drag_with_vertical_brake_v1` with its corner, and `record status`
// shows it. A Lab operation does not press it: `do --capture --record` on its step is refused
// with `record_gesture_execution_unsupported` before anything is pressed or recorded, a device
// frame is `record_step_click_not_executed` as for any declared click, and an offline frame
// closes the step. `record stop` declares it as the source click the Runtime reads, counts its
// 550 ms in the task timeout, records it in the provenance, admits the package, and its first
// decision is the kernel's brake drag from inside the click rectangle into the corner box and
// 100 px up, with the four fixed constants. (The `--brake-corner` shortcut has its test in
// actinglab's `record mark` flag tests.)
//
// Frames are synthetic: 192x160 pixels of flat gray with neutral noise patches; no OCR model is
// installed and no device is used. Requirement basis: plan-lab-l5 §2 BD, R1 to R5, with
// coordinator decision Q6.

use crate::codes::LabCode;
use crate::recording::crosscheck::admit;
use crate::recording::store::{old_record_path, recording_path};
use crate::{
    ClickSpec, LAB_RECORD_MARK_SCHEMA, LabOcr, MarkFamily, MarkOutcome, MarkRequest, MarkSpec,
    PlanClickRequest, PlannedSwipe, RecordPoint, RecordRect, RecordStartOptions, RecordStopOptions,
    RecordingLock, record_frame_preflight, record_mark, record_plan_click, record_start,
    record_status, record_stop,
};
use actingcommand_contract::{InputAction, LabError, LabErrorClass};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_execution_kernel::{OfflineDecision, simulate_contained_task};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_bd";
const TASK_ID: &str = "neutral_task";
const WIDTH: u32 = 192;
const HEIGHT: u32 = 160;
const BACKGROUND: u8 = 60;

// The template of step 1 and the template of step 2, each searched at its crop.
const BUTTON: RecordRect = RecordRect {
    x: 20,
    y: 20,
    width: 12,
    height: 12,
};
const NEXT: RecordRect = RecordRect {
    x: 100,
    y: 40,
    width: 12,
    height: 12,
};
// The brake drag of step 1: it presses in FROM and moves into CORNER, whose top is 120 px below
// the frame's top, so the brake 100 px up ends inside the frame.
const FROM: RecordRect = RecordRect {
    x: 140,
    y: 130,
    width: 16,
    height: 16,
};
const CORNER: RecordRect = RecordRect {
    x: 40,
    y: 120,
    width: 16,
    height: 16,
};

// A deterministic generator of neutral gray noise.
struct Noise(u64);

impl Noise {
    // A value in `low..=high`.
    fn between(&mut self, low: u8, high: u8) -> u8 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        low + ((self.0 >> 33) % (u64::from(high - low) + 1)) as u8
    }
}

// A gray image, row-major; it is drawn with equal red, green and blue.
#[derive(Clone)]
struct Gray {
    width: u32,
    height: u32,
    values: Vec<u8>,
}

impl Gray {
    fn from_fn(width: u32, height: u32, mut value: impl FnMut(u32, u32) -> u8) -> Self {
        let mut values = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            for x in 0..width {
                values.push(value(x, y));
            }
        }
        Self {
            width,
            height,
            values,
        }
    }

    fn frame() -> Self {
        Self::from_fn(WIDTH, HEIGHT, |_, _| BACKGROUND)
    }

    fn noise(seed: u64) -> Self {
        let mut noise = Noise(seed);
        Self::from_fn(12, 12, |_, _| noise.between(1, 255))
    }

    // `image` drawn with its top-left corner at the rectangle's.
    fn draw(mut self, at: RecordRect, image: &Gray) -> Self {
        for row in 0..image.height {
            for column in 0..image.width {
                self.values[((at.y as u32 + row) * self.width + at.x as u32 + column) as usize] =
                    image.values[(row * image.width + column) as usize];
            }
        }
        self
    }

    fn png(&self) -> Vec<u8> {
        let pixels = self
            .values
            .iter()
            .flat_map(|value| [*value; 3])
            .collect::<Vec<_>>();
        Frame::from_pixels(
            self.width,
            self.height,
            pixels,
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("fixture frame")
        .encode_png_fast()
        .expect("fixture PNG")
    }
}

fn rect(x: i32, y: i32, width: i32, height: i32) -> RecordRect {
    RecordRect {
        x,
        y,
        width,
        height,
    }
}

// Step 1's screen: the button; step 2's screen: the next mark.
fn first() -> Gray {
    Gray::frame().draw(BUTTON, &Gray::noise(0x0411_0bd1))
}

fn second() -> Gray {
    Gray::frame().draw(NEXT, &Gray::noise(0x0411_0bd2))
}

fn template(id: &str, crop: RecordRect) -> MarkSpec {
    MarkSpec {
        search: Some(crop),
        ..MarkSpec::region_only(id.to_owned(), MarkFamily::Template, crop)
    }
}

// The brake drag in the request form: `"click": {"region": FROM, "brake_corner": corner}`.
fn brake_drag(corner: RecordRect) -> ClickSpec {
    serde_json::from_value(json!({"region": FROM, "brake_corner": corner}))
        .expect("a brake drag click request")
}

// The gesture `recording.json`, the mark output and `record status` show for it.
fn declared_gesture() -> Value {
    json!({"kind": "single_touch_drag_with_vertical_brake_v1", "corner": CORNER})
}

// A state directory with an active session of `INSTANCE` and its Lab recording, before any
// step, without an OCR model. The lock is dropped before the directory.
struct Recording {
    lock: RecordingLock,
    ocr: LabOcr,
    temp: TempDir,
}

impl Recording {
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
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_lab_bd").expect("recording lock");
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

    // Writes `image` as the frame file `name` and returns its path.
    fn frame(&self, name: &str, image: &Gray) -> String {
        let path = self.temp.path().join(format!("{name}.png"));
        fs::write(&path, image.png()).expect("frame file");
        path.display().to_string()
    }

    fn mark(&self, request: MarkRequest) -> Result<MarkOutcome, LabError> {
        record_mark(&self.lock, &request, false, &self.ocr)
    }

    // Step 1 with its own frame, the button and `click`.
    fn first_step(&self, click: ClickSpec) -> MarkRequest {
        MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            frame: Some(self.frame("first", &first())),
            add: vec![template("ui/button", BUTTON)],
            click: Some(click),
            ..MarkRequest::default()
        }
    }

    // Step 1: the button with the brake drag into CORNER.
    fn mark_first(&self) -> MarkOutcome {
        self.mark(self.first_step(brake_drag(CORNER)))
            .unwrap_or_else(|error| panic!("step 1: {} {:?}", error.code, error.details))
    }

    // Step 2, given offline: the final screen with its own mark.
    fn mark_second(&self) -> MarkOutcome {
        self.mark(MarkRequest {
            schema_version: LAB_RECORD_MARK_SCHEMA.to_owned(),
            frame: Some(self.frame("second", &second())),
            add: vec![template("ui/next", NEXT)],
            ..MarkRequest::default()
        })
        .unwrap_or_else(|error| panic!("step 2: {} {:?}", error.code, error.details))
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

    // The files of the package `record stop` wrote (a template makes it a ZIP).
    fn package(&self, lab: &Value) -> BTreeMap<String, Vec<u8>> {
        let path = lab["path"].as_str().expect("package path");
        let bytes = fs::read(path).expect("package file");
        expand_content_container(&bytes, ContentContainer::Zip, ContainmentLimits::default())
            .expect("package container")
    }

    fn recording(&self) -> Value {
        serde_json::from_slice(&self.recording_bytes()).expect("recording")
    }

    fn recording_bytes(&self) -> Vec<u8> {
        fs::read(recording_path(&self.state(), RECORD_ID)).expect("recording file")
    }

    fn status(&self) -> Value {
        serde_json::to_value(record_status(&self.state(), INSTANCE).expect("record status"))
            .expect("status JSON")
    }
}

fn task(entries: &BTreeMap<String, Vec<u8>>) -> Value {
    let bytes = entries
        .get(&format!("resources/operations/{TASK_ID}/task.json"))
        .expect("task.json");
    serde_json::from_slice(bytes).expect("task JSON")
}

// The input the admitted package decides on `image` for its first step.
fn simulated(
    recording: &Recording,
    entries: &BTreeMap<String, Vec<u8>>,
    image: &Gray,
) -> InputAction {
    let prepared =
        admit(entries, &recording.ocr).unwrap_or_else(|error| panic!("admission: {}", error.code));
    let frame = Frame::from_png(image.png(), CaptureBackendName::AdbScreencap).expect("frame");
    let result = simulate_contained_task(&prepared, vec![frame]).expect("simulation");
    match result.decision {
        OfflineDecision::WouldClick {
            operation_label,
            action,
            ..
        } => {
            assert_eq!(operation_label, "step_01_click");
            action
        }
        other => panic!("not a click: {other:?}"),
    }
}

fn inside(x: i32, y: i32, rect: RecordRect) -> bool {
    x >= rect.x && y >= rect.y && x < rect.x + rect.width && y < rect.y + rect.height
}

// R1, R2: the request form `"click": {"region", "brake_corner"}` declares the brake drag on the
// click rectangle; the mark output, `recording.json` and `record status` keep the rectangle and
// show the gesture `single_touch_drag_with_vertical_brake_v1` with its corner.
#[test]
fn bd_a_brake_drag_is_declared_recorded_and_shown() {
    let recording = Recording::new();

    let marked = recording.mark_first();

    let click = serde_json::to_value(&marked.click).expect("click JSON");
    assert_eq!(click["rect"], json!(FROM));
    assert_eq!(click["gesture"], declared_gesture());
    let stored = recording.recording()["steps"][0]["click"].clone();
    assert_eq!(stored["rect"], json!(FROM));
    assert_eq!(stored["source"], "declared");
    assert_eq!(stored["gesture"], declared_gesture());
    let shown = recording.status()["steps"][0]["click"].clone();
    assert_eq!(shown["rect"], json!(FROM));
    assert_eq!(shown["outcome"], "declared");
    assert_eq!(shown["gesture"], declared_gesture());
}

// R1: what the Runtime refuses is refused when it is declared, each with validation_failed
// (exit 2), and nothing is written: a corner whose top is 99 px below the frame's top (the brake
// would leave the frame), a corner outside the frame, and the brake drag with a duration, a drag
// destination, a long press, a source mark or a followed template.
#[test]
fn bd_declarations_the_runtime_refuses_are_refused() {
    let recording = Recording::new();
    let before = recording.recording_bytes();
    for (case, click) in [
        ("a corner at y 99", brake_drag(rect(40, 99, 16, 16))),
        (
            "a corner outside the frame",
            brake_drag(rect(180, 150, 16, 16)),
        ),
        (
            "with a duration",
            ClickSpec {
                duration_ms: Some(550),
                ..brake_drag(CORNER)
            },
        ),
        (
            "with a drag destination",
            ClickSpec {
                drag_to: Some(rect(40, 60, 16, 16)),
                ..brake_drag(CORNER)
            },
        ),
        (
            "with a long press",
            ClickSpec {
                long_press: Some(rect(40, 60, 16, 16)),
                ..brake_drag(CORNER)
            },
        ),
        (
            "with a source mark",
            ClickSpec {
                from: Some("ui/button".to_owned()),
                ..brake_drag(CORNER)
            },
        ),
        (
            "with a followed template",
            ClickSpec {
                follow: Some("ui/button".to_owned()),
                ..brake_drag(CORNER)
            },
        ),
    ] {
        let error = recording.mark(recording.first_step(click)).expect_err(case);
        assert_eq!(error.code, "validation_failed", "{case}: {}", error.message);
        assert_eq!(error.class, LabErrorClass::UsageValidation, "{case}");
    }
    assert_eq!(recording.recording_bytes(), before, "nothing is written");
}

// R3: `do --capture --record` plans the open step's click before any Runtime request; on a step
// that declares a brake drag the plan is refused with record_gesture_execution_unsupported
// (exit 3, details step and gesture), whatever point, rectangle or swipe the command gives, and
// the recording is unchanged.
#[test]
fn bd_do_capture_record_refuses_the_gesture_before_any_runtime_request() {
    let recording = Recording::new();
    recording.mark_first();
    let before = recording.recording_bytes();

    for (case, request) in [
        ("no point", PlanClickRequest::default()),
        (
            "a point",
            PlanClickRequest {
                tap: Some(RecordPoint { x: 148, y: 138 }),
                ..PlanClickRequest::default()
            },
        ),
        (
            "a swipe",
            PlanClickRequest {
                swipe: Some(PlannedSwipe {
                    from: RecordPoint { x: 148, y: 138 },
                    to: RecordPoint { x: 48, y: 128 },
                    duration_ms: 200,
                }),
                ..PlanClickRequest::default()
            },
        ),
    ] {
        let error = record_plan_click(&recording.lock, &request).expect_err(case);
        assert_eq!(
            error.code,
            LabCode::RecordGestureExecutionUnsupported.as_str(),
            "{case}: {}",
            error.message
        );
        assert_eq!(error.class, LabErrorClass::SafetyBlocked, "{case}");
        assert_eq!(
            error.details,
            Some(json!({"step": 1, "gesture": declared_gesture()})),
            "{case}"
        );
    }
    assert_eq!(recording.recording_bytes(), before, "nothing is recorded");
}

// R3 to R5: a device frame on the step is record_step_click_not_executed, as for any declared
// click, and an offline frame closes it. `record stop` declares the step's click as the brake
// drag's source form with the fixed timings, counts its 550 ms in the task timeout (15000 step
// timeout + 550 + 200 post delay + 15000 arrival + 10000 margin) and records the gesture in the
// provenance; the kernel admits the package, and the first decision is the brake drag from
// inside the click rectangle into the corner box and 100 px straight up, with the constants.
#[test]
fn bd_the_step_closes_offline_and_the_package_declares_and_simulates_the_gesture() {
    let recording = Recording::new();
    recording.mark_first();

    let device = record_frame_preflight(&recording.lock).expect_err("the brake drag was not run");
    assert_eq!(
        device.code, "record_step_click_not_executed",
        "{}",
        device.message
    );
    let closed = recording.mark_second();
    assert_eq!(closed.closed_step, Some(1));
    assert_eq!(
        recording.recording()["steps"][0]["closed_by"],
        "offline_frame"
    );

    let lab = recording.stop();

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let entries = recording.package(&lab);
    let task = task(&entries);
    assert_eq!(
        task["operations"][0]["click"],
        json!({
            "kind": "single_touch_drag_with_vertical_brake_v1",
            "from": FROM,
            "corner": CORNER,
            "horizontal_duration_ms": 200,
            "corner_hold_ms": 150,
            "brake_distance_px": 100,
            "brake_duration_ms": 200
        })
    );
    assert_eq!(task["timeout_ms"], 40_750);
    assert_eq!(
        task["provenance"]["steps"][0]["click"]["gesture"],
        declared_gesture()
    );
    let InputAction::SingleTouchDragWithVerticalBrakeV1 {
        x1,
        y1,
        x2,
        y2,
        x3,
        y3,
        horizontal_duration_ms,
        corner_hold_ms,
        brake_distance_px,
        brake_duration_ms,
        ..
    } = simulated(&recording, &entries, &first())
    else {
        panic!("the first decision is not a brake drag");
    };
    assert!(inside(x1, y1, FROM), "({x1}, {y1})");
    assert!(inside(x2, y2, CORNER), "({x2}, {y2})");
    assert_eq!((x3, y3), (x2, y2 - 100));
    assert_eq!(
        (
            horizontal_duration_ms,
            corner_hold_ms,
            brake_distance_px,
            brake_duration_ms
        ),
        (200, 150, 100, 200)
    );
}
