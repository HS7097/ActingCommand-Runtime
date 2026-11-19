// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #479 F-3b (`contracts/display-adapter.md` section 13; register §1.487): the kernel's
//! frame gate and the Lab on a display within a pixel of the package's base's shape. A frame of
//! 1334x750 or 1366x768 for a 1280x720 package is brought to the base by the frame gate's own map
//! and its one resample (`lab_scene_on_base`, which a Lab operation's live frames, the client's
//! `--capture` forms and the stored scenes all take, on the gate a run's frame takes), and a Lab
//! press on such a frame takes the same map (`lab_display_map`); a stored scene of 1334x750 answers
//! on the base, naming `frame_coordinates` in the base form; a recording on such a display is
//! refused as before. The sizes' arithmetic is the adapter gate's (`gate_479_f3b` of the display
//! adapter): 1334x750 is 667/640 across and 25/24 down, 1366x768 683/640 and 16/15.
//!
//! Fix round 1 (the coordinator's work order `rd-1119/brief-479-f3b-fix1.md`, item 4): the two
//! paths of a recording that ask the map whether a size is the base scaled, and so count these
//! sizes as the map does. A recording whose header names no base size (made before 0.12.67) that
//! holds frames of 1334x750 is refused `record_size_not_base`, as one of 1920x1080 frames is; and
//! an ending's frame of 1334x750 given to a stopped recording of 1280x720 is converted to the
//! recording's size (#463 R2 fix 1, `scaled_to_recording`), as one of twice its size is.
//!
//! Frames are synthetic: one colour throughout, or gray screens of #463 R2's gate drawn at the
//! base's size (a flat gray, a noise patch, a square). No Runtime, no device, no OCR model, no
//! clock read by the tests; the session header is written as `record start` writes it.

use crate::recording::store::{old_record_path, recording_path};
use crate::{
    AttachFrameRequest, FrameCoordinates, LAB_RECORD_MARK_SCHEMA, LabOcr, MarkOutcome, MarkRequest,
    RECORDING_BASE_SIZE, RecordRect, RecordSize, RecordStartOptions, RecordStopOptions,
    RecordingLock, is_base_size, lab_display_map, lab_scene_on_base, record_attach_frame,
    record_frame_check, record_mark, record_size_check, record_start, record_stop, scene_in_base,
};
use actingcommand_contract::{DisplayMapKind, LabError};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_recognition::Scene;
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

const BASE: (u32, u32) = (1280, 720);
const COLOUR: [u8; 3] = [200, 40, 40];
const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0479_f3b";
const TASK_ID: &str = "neutral_task";
const STARTED_AT: u64 = 1_700_000_000_000;
/// The label the check names the frame it judges by.
const LABEL: &str = "the frame the path took";

// Fix round 1: #463 R2's screens and marks, at the base's size. The background gray; the seed of
// the next screen's noise patch (R2's); the ending's key, its mark and its square's gray.
const BACKGROUND: u8 = 60;
const NEXT_SEED: u64 = 0x0463_0002;
const SHORT_KEY: &str = "neutral_task_short";
const SHORT_MARK: &str = "ending/short";
const SHORT_GRAY: u8 = 180;

const fn rect(x: i32, y: i32, width: i32, height: i32) -> RecordRect {
    RecordRect {
        x,
        y,
        width,
        height,
    }
}

// The list's page area, its click and the next screen's noise patch (R2's), and the ending's
// square on the recording's 1280x720 screen.
const LIST_AREA: RecordRect = rect(2, 50, 4, 4);
const CLICK: RecordRect = rect(8, 8, 32, 32);
const PATCH: RecordRect = rect(40, 46, 12, 12);
const SHORT_AREA: RecordRect = rect(600, 500, 4, 4);
/// The ending's square as a 1334x750 display shows it: under 667/640 across and 25/24 down the
/// base's square covers the device's columns 625.3 to 629.5 and rows 520.8 to 525.0, and this
/// square covers them and more, so every base pixel of the square is the area average of device
/// pixels of the square's gray alone.
const SHORT_ON_1334X750: RecordRect = rect(620, 515, 16, 16);

/// A synthetic scene of `width` x `height`, `COLOUR` throughout.
fn scene(width: u32, height: u32) -> Scene {
    let pixels = (0..width * height)
        .flat_map(|_| COLOUR)
        .collect::<Vec<u8>>();
    Scene::from_rgb8(width, height, &pixels).expect("a synthetic scene")
}

// A deterministic generator of neutral gray noise (R2's).
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

/// The PNG of a gray screen of `width` x `height`, equal red, green and blue: `BACKGROUND`
/// throughout, with, when `patch`, the noise patch at `PATCH`, and each of `squares` in its gray.
fn gray_screen(width: u32, height: u32, patch: bool, squares: &[(RecordRect, u8)]) -> Vec<u8> {
    let columns = width as usize;
    let mut values = vec![BACKGROUND; columns * height as usize];
    if patch {
        let mut noise = Noise(NEXT_SEED);
        for row in 0..PATCH.height {
            for column in 0..PATCH.width {
                values[(PATCH.y + row) as usize * columns + (PATCH.x + column) as usize] =
                    noise.between(1, 199);
            }
        }
    }
    for (area, gray) in squares {
        for row in area.y..area.y + area.height {
            for column in area.x..area.x + area.width {
                values[row as usize * columns + column as usize] = *gray;
            }
        }
    }
    let pixels = values
        .iter()
        .flat_map(|value| [*value; 3])
        .collect::<Vec<_>>();
    Frame::from_pixels(
        width,
        height,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
    .encode_png_fast()
    .expect("fixture PNG")
}

/// The red value of the pixel (`x`, `y`) of `frame`, a gray screen.
fn gray_at(frame: &Frame, x: u32, y: u32) -> u8 {
    let channels = match frame.pixel_format {
        PixelFormat::Rgb8 => 3,
        PixelFormat::Rgba8 => 4,
    };
    frame.pixels[(y as usize * frame.width as usize + x as usize) * channels]
}

/// The values of a refusal's outcome, as its details carry them.
fn values(error: &LabError) -> Value {
    error.details.clone().unwrap_or(Value::Null)["outcome"]["values"].clone()
}

/// The values `record_display_not_base` and `record_size_not_base` carry for frames of `width` x
/// `height` and the base size 1280x720.
fn not_base_values(width: u32, height: u32) -> Value {
    json!({"frame_width": width, "frame_height": height, "base_width": 1280, "base_height": 720})
}

/// A state directory with an active session of `INSTANCE` and its Lab recording, without a frame
/// and without an OCR model, and the recording lock held as one command holds it (dropped before
/// the directory).
struct Bench {
    lock: RecordingLock,
    ocr: LabOcr,
    temp: TempDir,
}

impl Bench {
    /// The recording started as `record start` starts it, with `base` in its header (`None`: no
    /// base size, as a recording made before 0.12.67).
    fn started(base: Option<RecordSize>) -> Self {
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
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_479_f3b", "Lab-CLI")
            .expect("recording lock");
        let started = record_start(
            &lock,
            &RecordStartOptions {
                game: Some("neutral".to_owned()),
                server: Some("test".to_owned()),
                locale: Some("en-US".to_owned()),
                base_size: base,
                ..RecordStartOptions::default()
            },
        )
        .expect("record start");
        assert_eq!(started.status, "active", "{started:?}");
        let ocr = LabOcr::from_vision_root(&temp.path().join("no-vision"));
        Self { lock, ocr, temp }
    }

    fn state(&self) -> PathBuf {
        self.temp.path().join("state")
    }

    /// A device frame of `width` x `height`, a flat gray, as `capture --record` attaches it.
    fn attach(&self, width: u32, height: u32) {
        record_attach_frame(
            &self.lock,
            AttachFrameRequest {
                png: gray_screen(width, height, false, &[]),
                source: "lab_capture".to_owned(),
                runtime_artifact: None,
                capture_backend: Some("adb_screencap".to_owned()),
                freshness: None,
            },
        )
        .unwrap_or_else(|error| panic!("frame: {}: {}", error.code, error.message));
    }

    /// Writes `bytes` as the frame file `name` on the caller's side and returns its path.
    fn frame(&self, name: &str, bytes: &[u8]) -> String {
        let path = self.temp.path().join(format!("{name}.png"));
        fs::write(&path, bytes).expect("frame file");
        path.display().to_string()
    }

    /// `record mark` of the request `value`.
    fn mark(&self, mut value: Value) -> Result<MarkOutcome, LabError> {
        value["schema_version"] = json!(LAB_RECORD_MARK_SCHEMA);
        let request: MarkRequest = serde_json::from_value(value).expect("a record mark request");
        record_mark(&self.lock, &request, false, &self.ocr)
    }

    /// #463 R2's path at the base's size, recorded and stopped: step 1, the list (a flat gray)
    /// with its colour mark and its click, declared and not pressed; step 2, the next screen with
    /// its noise patch and its template, the usual ending. `record stop` generates the package,
    /// and the session file is then stopped as the command leaves it, so that the stopped
    /// recording takes `record mark` ("Marking after stop").
    fn stopped(&self) {
        let (width, height) = BASE;
        self.mark(json!({
            "frame": self.frame("list", &gray_screen(width, height, false, &[])),
            "add": [{"id": "state/list", "family": "color", "region": LIST_AREA}],
            "click": {"region": CLICK}
        }))
        .unwrap_or_else(|error| {
            panic!("record mark of step 1: {} {:?}", error.code, error.details)
        });
        self.mark(json!({
            "frame": self.frame("next", &gray_screen(width, height, true, &[])),
            "add": [{"id": "ui/next", "family": "template", "region": PATCH, "search": PATCH}]
        }))
        .unwrap_or_else(|error| {
            panic!("record mark of step 2: {} {:?}", error.code, error.details)
        });
        let stopped = record_stop(&self.lock, &RecordStopOptions::default(), &self.ocr)
            .unwrap_or_else(|error| panic!("record stop: {} {:?}", error.code, error.details));
        assert_eq!(
            stopped.lab_status.as_deref(),
            Some("generated"),
            "{:?}",
            stopped.lab
        );
        let path = old_record_path(&self.state(), INSTANCE);
        let mut session: Value =
            serde_json::from_slice(&fs::read(&path).expect("session file")).expect("session");
        session["status"] = json!("stopped");
        fs::write(&path, serde_json::to_vec(&session).expect("session JSON"))
            .expect("session file");
    }
}

/// Test 1, the resample: a 1334x750 and a 1366x768 frame of a 1280x720 package pass the frame
/// gate as a pure scale with the two factors and are brought to 1280x720 in one area-average
/// resample (a frame of one colour stays that colour); the map a Lab press on such a frame leaves
/// under is that map.
#[test]
fn f3b_the_frame_gate_brings_a_frame_within_a_pixel_of_the_base_to_the_base() {
    for (frame, across, down) in [
        ((1334, 750), (667, 640), (25, 24)),
        ((1366, 768), (683, 640), (16, 15)),
    ] {
        let reduced = lab_scene_on_base(BASE, &scene(frame.0, frame.1))
            .expect("the frame gate takes the frame")
            .expect("a frame of another size than the base is reduced");
        assert_eq!(reduced.map.kind(), DisplayMapKind::Scale, "{frame:?}");
        assert_eq!(
            (reduced.map.scale().num(), reduced.map.scale().den()),
            across,
            "{frame:?}"
        );
        assert_eq!(
            (
                reduced.map.height_scale().num(),
                reduced.map.height_scale().den()
            ),
            down,
            "{frame:?}"
        );
        assert_eq!(
            (reduced.scene.width(), reduced.scene.height()),
            BASE,
            "{frame:?}"
        );
        assert!(
            reduced
                .scene
                .rgb8_pixels()
                .chunks(3)
                .all(|pixel| pixel == COLOUR.as_slice()),
            "{frame:?}"
        );
        assert_eq!(lab_display_map(BASE, frame), Ok(reduced.map), "{frame:?}");
    }
}

/// Test 5: the Lab's reading of a stored scene of 1334x750 for a 1280x720 package
/// (`scene_in_base`, which every offline command takes) answers on the base, where it was refused
/// `lab_scene_size_unsupported` before: the scene reduced to 1280x720 and `frame_coordinates` in
/// the base form, reduced from 1334x750; the Lab's live frame of that size is the frame gate's
/// (test 1). A recording on such a display is refused as before: `record start`'s display check
/// (`is_base_size`) refuses it, and the judgement every recording path asks of a frame before it
/// sends or writes anything (`record_frame_check`) refuses the first frame of a recording made at
/// the base size, `record_display_not_base` with the frame's and the base's sizes.
#[test]
fn f3b_a_stored_scene_of_1334x750_answers_on_the_base_and_a_recording_still_refuses_it() {
    let (reduced, coordinates) = scene_in_base(Some(BASE), scene(1334, 750))
        .unwrap_or_else(|error| panic!("{}: {}", error.code, error.message));
    assert_eq!((reduced.width(), reduced.height()), BASE);
    assert_eq!(
        coordinates,
        Some(FrameCoordinates::observed(BASE, (1334, 750)))
    );
    assert_eq!(
        serde_json::to_value(coordinates).expect("frame_coordinates JSON"),
        json!({
            "space": "base",
            "base_pixels": [1280, 720],
            "reduced_from": [1334, 750],
            "orientation": "landscape",
            "applies_to": ["tap", "tap_rect", "swipe"]
        })
    );

    assert!(!is_base_size(1334, 750));
    let bench = Bench::started(Some(RECORDING_BASE_SIZE));
    let refused = record_frame_check(&bench.lock, (1334, 750), LABEL)
        .expect_err("a first frame of 1334x750 for a recording at the base size");
    assert_eq!(
        refused.code, "record_display_not_base",
        "{}",
        refused.message
    );
    assert_eq!(
        values(&refused),
        not_base_values(1334, 750),
        "{}",
        refused.message
    );
}

/// Fix round 1, the first path: a recording whose header names no base size (one made before
/// 0.12.67) takes its first frame's size, here 1334x750, as `capture --record` attaches it. Its
/// frames are judged by whether the map takes them for the base scaled (`recording_not_base`,
/// `larger_display_of_base_ratio`), and such a size now is: the recording's judgement
/// (`record_size_check`, which `record stop`, `record ready` and the paths that send ask) and a
/// frame of its own size (`record_frame_check`) are each refused `record_size_not_base`, with the
/// frames' size and the base size 1280x720.
#[test]
fn f3b_a_recording_without_a_base_size_holding_frames_of_1334x750_is_record_size_not_base() {
    let bench = Bench::started(None);
    bench.attach(1334, 750);
    let judged = record_size_check(&bench.lock).expect_err("a recording whose frames are 1334x750");
    assert_eq!(judged.code, "record_size_not_base", "{}", judged.message);
    assert_eq!(
        values(&judged),
        not_base_values(1334, 750),
        "{}",
        judged.message
    );
    let framed = record_frame_check(&bench.lock, (1334, 750), LABEL)
        .expect_err("a frame of a recording whose frames are 1334x750");
    assert_eq!(framed.code, "record_size_not_base", "{}", framed.message);
    assert_eq!(
        values(&framed),
        not_base_values(1334, 750),
        "{}",
        framed.message
    );
}

/// Fix round 1, the second path: a stopped recording of 1280x720 (the base size in its header)
/// takes an ending (`record mark --ending`) whose frame is a 1334x750 screen. The frame is a pure
/// scale of the recording by the map, so it is converted to the recording's 1280x720 as the
/// Runtime converts every frame (one area-average resample) before it is admitted: the ending's
/// frame is 1280x720 with `scaled_from` 1334x750, as the recording keeps it, its colour mark
/// self-tested on the converted frame, and the stored PNG holds the ending's square in its gray
/// inside the base's square and the background elsewhere.
#[test]
fn f3b_an_endings_frame_of_1334x750_is_converted_to_its_recordings_1280x720() {
    let bench = Bench::started(Some(RECORDING_BASE_SIZE));
    bench.stopped();
    let handset = gray_screen(1334, 750, false, &[(SHORT_ON_1334X750, SHORT_GRAY)]);
    let marked = bench
        .mark(json!({"ending": {
            "frame": bench.frame("handset", &handset),
            "outcome_key": SHORT_KEY,
            "add": [{"id": SHORT_MARK, "family": "color", "region": SHORT_AREA}]
        }}))
        .unwrap_or_else(|error| panic!("record mark --ending: {} {:?}", error.code, error.details));

    let frame = serde_json::to_value(&marked.frame).expect("frame JSON");
    assert_eq!(frame["width"], 1280, "{frame}");
    assert_eq!(frame["height"], 720, "{frame}");
    assert_eq!(
        frame["scaled_from"],
        json!({"width": 1334, "height": 750}),
        "{frame}"
    );
    let stored: Value = serde_json::from_slice(
        &fs::read(recording_path(&bench.state(), RECORD_ID)).expect("recording file"),
    )
    .expect("recording");
    assert_eq!(stored["endings"][0]["frames"][0], frame, "{stored}");
    assert_eq!(
        stored["endings"][0]["marks"][0]["self_test"]["status"], "passed",
        "{stored}"
    );
    let kept = Frame::from_png(
        fs::read(frame["path"].as_str().expect("frame path")).expect("stored frame"),
        CaptureBackendName::AdbScreencap,
    )
    .expect("stored PNG");
    assert_eq!((kept.width, kept.height), BASE);
    for (x, y, gray) in [
        (600, 500, SHORT_GRAY),
        (603, 503, SHORT_GRAY),
        (2, 50, BACKGROUND),
        (1279, 719, BACKGROUND),
    ] {
        assert_eq!(gray_at(&kept, x, y), gray, "({x}, {y})");
    }
}
