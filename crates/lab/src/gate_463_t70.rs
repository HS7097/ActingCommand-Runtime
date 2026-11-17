// SPDX-License-Identifier: AGPL-3.0-only

// The 0.12.70 train (register §1.449, "The train of 0.12.70"; contracts/lab-recording.md, "Key
// steps" and "Folding swipes into a scrolling search"): where the key step of a recording (#463
// KR-1) meets the fold of swipes into a scrolling search (#463 S2-8') and the scrolling choice
// (#463 S2-9a, S2-9b). Recordings are made as stage one makes them (taps and swipes through
// `record_press` with a fake Runtime, its captures taking the scripted screens and the caller's
// clock moving only by its waits), on S2-9b's synthetic list: 128x128 screens, a page square of one
// colour at (0, 0, 16, 16) and, on the list screens, a list area at rows 20 to 99 that shows the
// content rows `offset` to `offset + 79`; eleven content rows 12 px apart, each an anchor of noise
// and a name marker pixel the stand-in name reader reads; a start mark of noise at content rows 0
// to 3. Steps: 1 the tap on the start screen that opens the list at offset 0, 2 and 3 the forward
// swipes to offsets 24 and 48, 4 the press on `india` (row 8), 5 the detail screen, and (the joined
// recording) a press on the detail screen that opens 6 the home screen. No Runtime, no device, no
// OCR model but the stand-in reader; the tests read no clock and never sleep.
// - `t70_a_key_on_a_folded_swipe_is_the_folds_own_refusal` (1)
// - `t70_a_press_that_holds_a_fold_refuses_a_key_with_its_own_word` (2)
// - `t70_a_fold_over_a_key_step_or_onto_one_is_refused_by_the_folds_range_rule` (3)
// - `t70_the_guard_a_key_gives_back_is_a_mark_in_use_while_the_key_stands` (4)
// - `t70_status_review_ready_and_the_cases_answer_a_key_a_folded_select_and_a_press` (5)
// - `t70_the_archive_keeps_the_folded_swipes_and_the_key_steps_row` (6)

use crate::codes::{KeyProblem, LabCode, ScanProblem};
use crate::recording::frames::decode_frame;
use crate::recording::store::{lab_dir, old_record_path, recording_path};
use crate::{
    AfterCapture, AttachFrameRequest, CapturedFrame, ClickEffect, ClickPlan, CommitClickRequest,
    LAB_RECORD_MARK_SCHEMA, LabOcr, MarkOutcome, MarkRequest, PlanClickRequest, PlannedSwipe,
    PointPress, RecordArchiveOptions, RecordFramePorts, RecordPoint, RecordPressRequest,
    RecordRect, RecordReviewRequest, RecordStartOptions, RecordStopOptions, RecordingLock,
    RegressionVerdict, record_archive, record_mark, record_press, record_ready, record_review,
    record_start, record_status, record_stop, run_regression_case,
};
use actingcommand_contract::{InputFrameReference, LabError, LabResult};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, PackRect, VisionProvider, VisionProviderError, VisionProviderErrorCode,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0463_t70";
const TASK_ID: &str = "neutral_task";
/// The package's id, `<game>.<server>.<task_id>`.
const PACKAGE_ID: &str = "neutral.test.neutral_task";
const STARTED_AT: u64 = 1_700_000_000_000;
const WIDTH: u32 = 128;
const HEIGHT: u32 = 128;
const GRAY: [u8; 3] = [60, 60, 60];
const RED: [u8; 3] = [230, 20, 20];
const GREEN: [u8; 3] = [20, 230, 20];
const BLUE: [u8; 3] = [20, 20, 230];
const YELLOW: [u8; 3] = [230, 230, 20];
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
/// The back key, the vocabulary's one word.
const BACK: &str = "back";
/// The self-checks of `record stop`, in the order they run.
const EVERY_CHECK: [&str; 8] = [
    "frames",
    "admission",
    "first_decision",
    "cross_check",
    "endings",
    "selects",
    "final_package",
    "cases",
];

const fn rect(x: i32, y: i32, width: i32, height: i32) -> RecordRect {
    RecordRect {
        x,
        y,
        width,
        height,
    }
}

/// Each screen's page square: red on the start screen, green on the list, yellow on the detail,
/// blue on the home screen.
const SQUARE: RecordRect = rect(0, 0, 16, 16);
/// Test 4's second mark of step 1, on the gray of the start screen.
const CORNER: RecordRect = rect(100, 100, 16, 16);
/// The list's area: the frame's rows 20 to 99.
const LIST_TOP: u32 = 20;
const LIST_HEIGHT: u32 = 80;
const LIST: RecordRect = rect(0, 20, 128, 80);
/// The rows of the content: eleven, 12 px apart, row 0 at content row 4.
const ROWS: u32 = 11;
const PITCH: u32 = 12;
const FIRST_ROW: u32 = 4;
/// The names the list may show, the eleven of its rows and `lima`, which no row shows.
const NAMES: [&str; 12] = [
    "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliet",
    "kilo", "lima",
];
const ALIASES: [&str; 12] = [
    "ALPHA", "BRAVO", "CHARLIE", "DELTA", "ECHO", "FOXTROT", "GOLF", "HOTEL", "INDIA", "JULIET",
    "KILO", "LIMA",
];
/// The start and end marks: noise at x 60 to 75, content rows 0 to 3 and 136 to 141.
const MARK_X: u32 = 60;
const MARK_WIDTH: u32 = 16;
const START_ROWS: u32 = 4;
const END_TOP: u32 = 136;
const END_ROWS: u32 = 6;

/// The agent's regions on the list's first screen (offset 0): row 0 at frame row 24.
const ROW: RecordRect = rect(2, 24, 40, 8);
const ANCHOR: RecordRect = rect(4, 24, 8, 6);
const NAME: RecordRect = rect(18, 24, 16, 6);
const START_MARK: RecordRect = rect(60, 20, 16, 4);

/// The tap on the start screen, the press on the row `india` (row 8, at frame rows 72 to 79 on the
/// screen at offset 48), and the press on the detail screen the key replaces.
const START_POINT: (i32, i32) = (100, 110);
const WANTED_POINT: (i32, i32) = (10, 76);
const KEY_POINT: (i32, i32) = (64, 110);

/// A swipe by its two points: up the list by 24 px.
type Swipe = ((i32, i32), (i32, i32));
const FORWARD: Swipe = ((64, 80), (64, 56));

// Deterministic pixel noise: a shifted copy of it correlates with it near zero.
fn noise(x: u32, y: u32) -> [u8; 3] {
    let mut state = u64::from(y * 256 + x).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x2545_f491;
    let mut next = || {
        state ^= state >> 33;
        state = state.wrapping_mul(0xff51_afd7_ed55_8ccd);
        state ^= state >> 33;
        (state & 0xff) as u8
    };
    [next(), next(), next()]
}

// Whether `area` covers the pixel at `(x, y)`.
fn covers(area: RecordRect, x: u32, y: u32) -> bool {
    let (column, row) = (i64::from(x), i64::from(y));
    column >= i64::from(area.x)
        && row >= i64::from(area.y)
        && column < i64::from(area.x) + i64::from(area.width)
        && row < i64::from(area.y) + i64::from(area.height)
}

// The PNG of a screen whose pixels `pixel` gives.
fn image(pixel: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            pixels.extend(pixel(x, y));
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

// A gray screen with its page square in `color`.
fn squared(color: [u8; 3]) -> Vec<u8> {
    image(|x, y| if covers(SQUARE, x, y) { color } else { GRAY })
}

fn start() -> Vec<u8> {
    squared(RED)
}

fn detail() -> Vec<u8> {
    squared(YELLOW)
}

fn home() -> Vec<u8> {
    squared(BLUE)
}

// The list scrolled by `offset` px.
fn list_at(offset: u32) -> Vec<u8> {
    image(|x, y| {
        if covers(SQUARE, x, y) {
            return GREEN;
        }
        if !(LIST_TOP..LIST_TOP + LIST_HEIGHT).contains(&y) {
            return GRAY;
        }
        let row = y - LIST_TOP + offset;
        if (MARK_X..MARK_X + MARK_WIDTH).contains(&x) {
            if row < START_ROWS {
                return noise(x - MARK_X + 100, row + 100);
            }
            if (END_TOP..END_TOP + END_ROWS).contains(&row) {
                return noise(x - MARK_X + 150, row - END_TOP + 150);
            }
        }
        for k in 0..ROWS {
            let top = FIRST_ROW + PITCH * k;
            if (top..top + 6).contains(&row) && (4..12).contains(&x) {
                return noise(x - 4, row - top);
            }
            if row == top + 2 && x == 20 {
                return [200 + u8::try_from(k).expect("a row"); 3];
            }
        }
        GRAY
    })
}

// The pixel at `(x, y)` of the PNG `png`, a case's scene.
fn scene_pixel(png: &[u8], x: u32, y: u32) -> [u8; 3] {
    let frame = decode_frame(png.to_vec(), "a scene of the test")
        .expect("a PNG")
        .frame;
    let channels = match frame.pixel_format {
        PixelFormat::Rgb8 => 3,
        PixelFormat::Rgba8 => 4,
    };
    let at = (y as usize * frame.width as usize + x as usize) * channels;
    [frame.pixels[at], frame.pixels[at + 1], frame.pixels[at + 2]]
}

// Reads one block per name marker pixel inside the requested region, its text the alias of the
// name its grey value stands for, at confidence 0.9; a region without one reads nothing.
#[derive(Debug)]
struct RowNames;

impl VisionProvider for RowNames {
    fn require_ocr_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Ok(())
    }

    fn require_nn_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }

    fn read_text(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        let region = request.region;
        let (right, bottom) = (region.x + region.width, region.y + region.height);
        let mut blocks = Vec::new();
        for y in region.y..bottom {
            for x in region.x..right {
                let index = (y as usize * request.frame.width as usize + x as usize) * 3;
                let Some(alias) = request
                    .frame
                    .rgb8_pixels
                    .get(index)
                    .and_then(|red| red.checked_sub(200))
                    .and_then(|k| ALIASES.get(usize::from(k)))
                else {
                    continue;
                };
                blocks.push(OcrProviderTextBlock {
                    text: (*alias).to_owned(),
                    rect: PackRect {
                        x,
                        y,
                        width: 1,
                        height: 1,
                    },
                    confidence: Some(0.9),
                });
            }
        }
        let text = blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            confidence: (!blocks.is_empty()).then_some(0.9),
            text,
            blocks,
            detection: None,
        })
    }

    fn classify(
        &self,
        _request: NnProviderRequest<'_>,
    ) -> Result<NnProviderResult, VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }
}

// The colour mark `id` over `area`.
fn color(id: &str, area: RecordRect) -> Value {
    json!({"id": id, "family": "color", "region": area})
}

fn request(mut value: Value) -> MarkRequest {
    value["schema_version"] = json!(LAB_RECORD_MARK_SCHEMA);
    serde_json::from_value(value).expect("a record mark request")
}

// The scrolling select of the second kind over the list, straight to the wanted value's default
// `india`, the row the demonstration pressed, without an end mark.
fn straight() -> Value {
    json!({
        "rows": {"row": ROW, "anchor": ANCHOR, "name": NAME},
        "scan": {"list": LIST, "start": START_MARK, "stop": "wanted"},
        "read": {"languages": ["en"], "timeout_ms": 1000, "minimum_confidence_milli": 800,
                 "model_ref": "neutral-ocr", "model_sha256": MODEL_SHA256},
        "names": NAMES
            .iter()
            .zip(ALIASES)
            .map(|(id, alias)| json!({"id": id, "aliases": [alias]}))
            .collect::<Vec<_>>(),
        "match": {"max_distance": 1, "minimum_margin": 1},
        "wanted": {"name": "list_item", "default": "india"}
    })
}

// The swipes of steps 2 and 3 folded into the press of step 4.
fn fold() -> Value {
    json!({"step_action": {"kind": "fold_scan", "step": 4, "first": 2}})
}

// The back key declared on step `step` in place of its press.
fn key_on(step: u32) -> Value {
    json!({"step": step, "key": {"key": BACK}, "replace_click": true})
}

fn point(at: (i32, i32)) -> RecordPoint {
    RecordPoint { x: at.0, y: at.1 }
}

// What the command hands the recording for a performed input.
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

/// The fake Runtime's captures and the caller's clock, which moves only by the waits.
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

// The screen after a press as one capture 800 ms after it, without a stillness test.
fn one_capture_after() -> AfterCapture {
    AfterCapture {
        still_ms: Some(0),
        ..AfterCapture::for_press(Some(800))
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

// The ids of `task`'s operations, in order.
fn operation_ids(task: &Value) -> Vec<String> {
    task["operations"]
        .as_array()
        .expect("operations")
        .iter()
        .map(|operation| operation["id"].as_str().expect("an id").to_owned())
        .collect()
}

// The item of `rows` whose `key` is `step`.
fn row<'v>(rows: &'v Value, key: &str, step: u32) -> &'v Value {
    rows.as_array()
        .unwrap_or_else(|| panic!("no rows in {rows}"))
        .iter()
        .find(|item| item[key] == step)
        .unwrap_or_else(|| panic!("no step {step} in {rows}"))
}

// Step `step` of `recording.json`.
fn step_of(recording: &Value, step: u32) -> &Value {
    row(&recording["steps"], "index", step)
}

// The details of a refusal with `code` whose value `key` is `word`, in the details and in the
// outcome's values.
fn refusal(error: LabError, code: &str, key: &str, word: &str) -> Value {
    assert_eq!(error.code, code, "{}: {:?}", error.message, error.details);
    let details = error.details.expect("details");
    assert_eq!(details[key], word, "{details}");
    assert_eq!(details["outcome"]["values"][key], word, "{details}");
    details
}

/// A state directory with an active session of `INSTANCE` and its Lab recording, its game, server
/// and locale known, with the stand-in name reader. The lock is dropped before the directory.
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
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_463_t70", "Lab-CLI")
            .expect("recording lock");
        let bench = Self {
            lock,
            ocr: LabOcr::with_provider(Arc::new(RowNames)),
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

    // `record_press` of `pressed`, the fake Runtime performing the input and its captures taking
    // `shots`: the screen before the press, then the screen after it.
    fn press_with(&self, pressed: &RecordPressRequest, shots: [Vec<u8>; 2]) {
        let mut device = Screens {
            shots: shots.into(),
            taken: 0,
            now: Duration::ZERO,
        };
        let mut send = |_: &ClickPlan, _: InputFrameReference| -> LabResult<CommitClickRequest> {
            Ok(receipt())
        };
        if let Err(error) = record_press(&self.lock, pressed, &mut send, &mut device) {
            panic!(
                "the press: {}: {} {:?}",
                error.code, error.message, error.details
            );
        }
    }

    // A point press recorded at `at` on the first of `shots`.
    fn press_at(&self, at: (i32, i32), shots: [Vec<u8>; 2]) {
        let pressed = RecordPressRequest {
            click: PlanClickRequest::default(),
            point_press: Some(PointPress::Tap { point: point(at) }),
            after: one_capture_after(),
        };
        self.press_with(&pressed, shots);
    }

    // A drag of the list over 300 ms recorded on the first of `shots`.
    fn drag_list(&self, (from, to): Swipe, shots: [Vec<u8>; 2]) {
        let pressed = RecordPressRequest {
            click: PlanClickRequest {
                swipe: Some(PlannedSwipe {
                    from: point(from),
                    to: point(to),
                    duration_ms: 300,
                }),
                ..PlanClickRequest::default()
            },
            point_press: None,
            after: one_capture_after(),
        };
        self.press_with(&pressed, shots);
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

    // `record mark` of `value`, which must be refused, every file as it was; the refusal.
    fn refused(&self, value: Value) -> LabError {
        let before = self.files();
        let error = self.mark(value.clone()).expect_err("a refused record mark");
        assert_eq!(self.files(), before, "a file changed: {value}");
        error
    }

    fn files(&self) -> Vec<(String, Vec<u8>)> {
        files_under(&lab_dir(&self.state(), RECORD_ID))
    }

    // The bytes of `recording.json`.
    fn recording_bytes(&self) -> Vec<u8> {
        fs::read(recording_path(&self.state(), RECORD_ID)).expect("recording.json")
    }

    // `recording.json` as JSON.
    fn recording(&self) -> Value {
        serde_json::from_slice(&self.recording_bytes()).expect("recording JSON")
    }

    // `record status`'s `lab`.
    fn status(&self) -> Value {
        let status = record_status(&self.state(), INSTANCE)
            .unwrap_or_else(|error| panic!("record status: {}: {}", error.code, error.message));
        serde_json::to_value(&status).expect("record status JSON")
    }

    // `record review --no-preview` of every effective step.
    fn review(&self) -> Value {
        let asked = RecordReviewRequest {
            record_id: None,
            step: None,
            no_preview: true,
        };
        let outcome = record_review(
            &self.state(),
            INSTANCE,
            &asked,
            &RecordStopOptions::default(),
            &self.ocr,
        )
        .unwrap_or_else(|error| panic!("record review: {}: {}", error.code, error.message));
        serde_json::to_value(&outcome).expect("record review JSON")
    }

    // `record ready`, read as the command reads it.
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

    // `record stop`, which must stand: the `lab` output with its status.
    fn stopped(&self) -> Value {
        match record_stop(&self.lock, &RecordStopOptions::default(), &self.ocr) {
            Ok(outcome) => {
                let mut lab = serde_json::to_value(&outcome.lab).expect("lab JSON");
                lab["lab_status"] = json!(outcome.lab_status);
                lab
            }
            Err(error) => panic!(
                "record stop: {}: {} {:?}",
                error.code, error.message, error.details
            ),
        }
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

// The presses of stage one: step 1 the tap that opens the list at offset 0, steps 2 and 3 the
// forward swipes to offsets 24 and 48, step 4 the press on `india` that opens the detail screen
// (step 5); with `to_home`, a press on the detail screen that opens the home screen (step 6).
fn pressed(to_home: bool) -> Bench {
    let bench = Bench::new();
    bench.press_at(START_POINT, [start(), list_at(0)]);
    for (from, to) in [(0, 24), (24, 48)] {
        bench.drag_list(FORWARD, [list_at(from), list_at(to)]);
    }
    bench.press_at(WANTED_POINT, [list_at(48), detail()]);
    if to_home {
        bench.press_at(KEY_POINT, [detail(), home()]);
    }
    bench
}

// The recording of [`pressed`] without the press on the detail screen, each page marked (steps 1,
// 4 and 5); nothing folded.
fn listed() -> Bench {
    let bench = pressed(false);
    bench.marked(json!({"step": 1, "add": [color("ui/start", SQUARE)]}));
    bench.marked(json!({"step": 4, "add": [color("ui/list", SQUARE)]}));
    bench.marked(json!({"step": 5, "add": [color("ui/detail", SQUARE)]}));
    bench
}

// The joined recording: the presses of [`pressed`] to the home screen, each page marked (steps 1,
// 4, 5 and 6), the swipes of steps 2 and 3 folded into the press of step 4, the scrolling select
// of the second kind on step 4, and the back key on step 5 in place of its press, guarded by its
// page's colour mark by the generator's rule.
fn joined() -> Bench {
    let bench = pressed(true);
    bench.marked(json!({"step": 1, "add": [color("ui/start", SQUARE)]}));
    bench.marked(json!({"step": 4, "add": [color("ui/list", SQUARE)]}));
    bench.marked(json!({"step": 5, "add": [color("ui/detail", SQUARE)]}));
    bench.marked(json!({"step": 6, "add": [color("ui/home", SQUARE)]}));
    bench.marked(fold());
    bench.marked(json!({"step": 4, "select": straight()}));
    bench.marked(key_on(5));
    bench
}

// The train's rule 1: a key on a swipe folded into the scrolling search of step 4 is answered by
// the fold's own refusal, `record_scan_invalid` with `step_in_fold` naming the press step, with
// `replace_click` or without, nothing written: the key's check of a dropped or converted step
// reads the path the recording kept, which a folded swipe is on, and tells it from a dropped step:
// the last step dropped, a key on it is still the key's own `step_not_effective`.
#[test]
fn t70_a_key_on_a_folded_swipe_is_the_folds_own_refusal() {
    let bench = listed();
    bench.marked(fold());
    for swipe in [2, 3] {
        for declared in [key_on(swipe), json!({"step": swipe, "key": {"key": BACK}})] {
            let details = refusal(
                bench.refused(declared),
                LabCode::RecordScanInvalid.as_str(),
                "scan_problem",
                ScanProblem::StepInFold.as_str(),
            );
            assert_eq!(details["step"], swipe, "{details}");
            assert_eq!(details["fold_step"], 4, "{details}");
        }
    }
    bench.marked(json!({"step_action": {"kind": "drop_step", "step": 5}}));
    let details = refusal(
        bench.refused(key_on(5)),
        LabCode::RecordKeyInvalid.as_str(),
        "key_problem",
        KeyProblem::StepNotEffective.as_str(),
    );
    assert_eq!(details["step"], 5, "{details}");
}

// The train's rule 2: the press step that holds the fold refuses a key with a word of its own under
// the key's refusal, `record_key_invalid` with `holds_fold` (its press is the press of a scrolling
// search), with `replace_click` or without, the fold's first step in the details, nothing written;
// once its scrolling select reads the fold, the key is refused as on every step that holds a
// choice, `holds_choice`.
#[test]
fn t70_a_press_that_holds_a_fold_refuses_a_key_with_its_own_word() {
    let bench = listed();
    bench.marked(fold());
    for declared in [key_on(4), json!({"step": 4, "key": {"key": BACK}})] {
        let details = refusal(
            bench.refused(declared),
            LabCode::RecordKeyInvalid.as_str(),
            "key_problem",
            KeyProblem::HoldsFold.as_str(),
        );
        assert_eq!(details["step"], 4, "{details}");
        assert_eq!(details["fold_first"], 2, "{details}");
    }
    let selected = bench.marked(json!({"step": 4, "select": straight()}));
    assert!(selected.select.is_some(), "the scrolling select stands");
    let details = refusal(
        bench.refused(key_on(4)),
        LabCode::RecordKeyInvalid.as_str(),
        "key_problem",
        KeyProblem::HoldsChoice.as_str(),
    );
    assert_eq!(details["step"], 4, "{details}");
}

// The train's rule 3: the fold's own rules refuse a key on either side of it, nothing written. The
// swipe of step 3 replaced by the key: `fold_scan` of step 4 from step 2 is `fold_range_invalid` at
// step 3 (no executed swipe); the key taken back, the same range folds. The press of step 4
// replaced by the key: the fold onto it is `fold_range_invalid` at step 4 (no executed tap).
#[test]
fn t70_a_fold_over_a_key_step_or_onto_one_is_refused_by_the_folds_range_rule() {
    let ranged = listed();
    ranged.marked(key_on(3));
    let details = refusal(
        ranged.refused(fold()),
        LabCode::RecordScanInvalid.as_str(),
        "scan_problem",
        ScanProblem::FoldRangeInvalid.as_str(),
    );
    assert_eq!(details["step"], 4, "{details}");
    assert_eq!(details["first"], 2, "{details}");
    assert_eq!(details["at"], 3, "{details}");
    ranged.marked(json!({"step": 3, "key": false}));
    assert_eq!(ranged.marked(fold()).status, "scan_folded");

    let onto = listed();
    onto.marked(key_on(4));
    let details = refusal(
        onto.refused(fold()),
        LabCode::RecordScanInvalid.as_str(),
        "scan_problem",
        ScanProblem::FoldRangeInvalid.as_str(),
    );
    assert_eq!(details["step"], 4, "{details}");
    assert_eq!(details["at"], 4, "{details}");
}

// The train's rule 4: step 1's press is guarded by `ui/start`; its key names `ui/corner`, and keeps
// `ui/start` as the guard it gives back. While the key stands neither mark can be removed: `ui/start`
// is refused as a mark in use is (`record_mark_in_use`, nothing written), as `ui/corner`, the click
// guard, is. Taken back, the key gives `ui/start` back as the click guard, and `ui/corner` is an
// ordinary mark again, which is removed.
#[test]
fn t70_the_guard_a_key_gives_back_is_a_mark_in_use_while_the_key_stands() {
    let bench = Bench::new();
    bench.press_at(START_POINT, [start(), list_at(0)]);
    bench.marked(json!({"step": 1, "add": [
        color("ui/start", SQUARE),
        color("ui/corner", CORNER)
    ]}));
    bench.marked(json!({"step": 1, "click_guard": "ui/start"}));
    let mut declared = key_on(1);
    declared["click_guard"] = json!("ui/corner");
    bench.marked(declared);
    let recording = bench.recording();
    let keyed = step_of(&recording, 1);
    assert_eq!(keyed["click_guard"], "ui/corner", "{keyed}");
    assert_eq!(keyed["key"]["replaced_guard"], "ui/start", "{keyed}");
    for id in ["ui/start", "ui/corner"] {
        let error = bench.refused(json!({"step": 1, "remove": [id]}));
        assert_eq!(error.code, "record_mark_in_use", "{id}: {}", error.message);
        let details = error.details.expect("details");
        assert_eq!(details["id"], id, "{details}");
    }

    bench.marked(json!({"step": 1, "key": false}));
    let recording = bench.recording();
    let restored = step_of(&recording, 1);
    assert_eq!(restored["click_guard"], "ui/start", "{restored}");
    assert_eq!(restored.get("key"), None, "{restored}");
    let removed = bench.marked(json!({"step": 1, "remove": ["ui/corner"]}));
    assert_eq!(removed.removed, ["ui/corner"]);
}

// The train's rule 5: one recording holds an ordinary press (step 1), a scrolling select over the
// swipes folded into it (step 4, the swipes 2 and 3) and a key step (step 5). `record status`
// numbers the effective steps 1, 4, 5 and 6 as 1 to 4 and the swipes none, and shows each step as
// the recording holds it; `record review` answers the four, the press as a tap, the select's sheet
// with its folded swipes and its scan, the key with the press it replaced; `record ready` is ready,
// every self-check run and complete, step 1's point press its one `press_point_only`. `record stop`
// writes `step_01_click`, `step_02_select` and `step_03_key` and the cases `step_01`, `step_02`
// (the select's, on the list's first screen), `step_03` (the key step's) and `final`, none for a
// folded swipe, each proven and passing when read from the disk and run on the written package;
// stopped again after a mark that changes nothing, the package is the one generated before, but for
// its generation time.
#[test]
fn t70_status_review_ready_and_the_cases_answer_a_key_a_folded_select_and_a_press() {
    let bench = joined();
    let recording = bench.recording();
    let replaced = step_of(&recording, 5)["key"]["replaced"].clone();
    assert_eq!(replaced["source"], "point", "{recording}");
    assert_eq!(replaced["execution"]["effect"], "performed", "{recording}");

    let status = bench.status();
    let numbered = status["steps"]
        .as_array()
        .expect("status steps")
        .iter()
        .map(|item| (item["index"].clone(), item["artifact_step"].clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        numbered,
        [
            (json!(1), json!(1)),
            (json!(2), Value::Null),
            (json!(3), Value::Null),
            (json!(4), json!(2)),
            (json!(5), json!(3)),
            (json!(6), json!(4))
        ],
        "{status}"
    );
    let tapped = row(&status["steps"], "index", 1);
    assert_eq!(tapped["click"]["source"], "point", "{tapped}");
    assert_eq!(tapped["click"]["executed"], true, "{tapped}");
    for swipe in [2, 3] {
        let item = row(&status["steps"], "index", swipe);
        assert_eq!(item["folded_into"], 4, "{item}");
    }
    let selecting = row(&status["steps"], "index", 4);
    assert_eq!(selecting["scan_fold"]["first"], 2, "{selecting}");
    assert_eq!(
        selecting["select"]["declaration"]["scan"]["stop"], "wanted",
        "{selecting}"
    );
    assert_eq!(selecting["click"]["source"], "select", "{selecting}");
    let keyed = row(&status["steps"], "index", 5);
    assert_eq!(keyed["key"]["key"], BACK, "{keyed}");
    assert_eq!(keyed["key"]["replaced"], replaced, "{keyed}");
    assert_eq!(keyed["click"], Value::Null, "{keyed}");
    for step in [1, 4, 6] {
        let item = row(&status["steps"], "index", step);
        assert_eq!(item.get("key"), None, "{item}");
    }

    let review = bench.review();
    assert_eq!(review["steps_total"], 4, "{review}");
    let sheets = review["steps"].as_array().expect("reviewed steps");
    assert_eq!(
        sheets
            .iter()
            .map(|sheet| (sheet["step"].clone(), sheet["kind"].clone()))
            .collect::<Vec<_>>(),
        [
            (json!(1), json!("tap")),
            (json!(4), json!("tap")),
            (json!(5), json!("key")),
            (json!(6), json!("none"))
        ],
        "{review}"
    );
    for sheet in sheets {
        assert_eq!(sheet["missing"], json!([]), "{sheet}");
    }
    assert_eq!(
        row(&review["steps"], "step", 1)["press"]["source"],
        "point",
        "{review}"
    );
    let sheet = row(&review["steps"], "step", 4);
    assert_eq!(sheet["press"]["source"], "select", "{sheet}");
    assert_eq!(
        sheet["folded"]
            .as_array()
            .expect("the folded swipes")
            .iter()
            .map(|swipe| (swipe["step"].clone(), swipe["direction"].clone()))
            .collect::<Vec<_>>(),
        [(json!(2), json!("forward")), (json!(3), json!("forward"))],
        "{sheet}"
    );
    assert_eq!(sheet["scan"]["scan"]["stop"], "wanted", "{sheet}");
    let sheet = row(&review["steps"], "step", 5);
    assert_eq!(
        sheet["press"],
        json!({"key": BACK,
               "replaced": {"point": point(KEY_POINT),
                            "rect": rect(KEY_POINT.0, KEY_POINT.1, 1, 1),
                            "source": "point", "outcome": "executed"},
               "source": "declared", "outcome": "declared"}),
        "{sheet}"
    );
    assert_eq!(
        sheet["before"]["frame_id"], replaced["execution"]["before_frame"],
        "{sheet}"
    );
    assert_eq!(
        sheet["after"]["frame_id"], replaced["execution"]["after_frame"],
        "{sheet}"
    );
    assert_eq!(sheet["after"]["step"], 6, "{sheet}");
    assert_eq!(
        row(&review["steps"], "step", 6).get("press"),
        None,
        "{review}"
    );

    let ready = bench.ready();
    assert_eq!(ready["status"], "ready", "{ready}");
    assert_eq!(ready["missing"], json!([]), "{ready}");
    assert_eq!(
        ready["checks"]
            .as_array()
            .expect("the checks")
            .iter()
            .map(|run| (run["check"].clone(), run["complete"].clone()))
            .collect::<Vec<_>>(),
        EVERY_CHECK.map(|check| (json!(check), json!(true))),
        "{ready}"
    );
    assert_eq!(ready["checks_after"], json!([]), "{ready}");
    assert_eq!(
        ready["warnings"]
            .as_array()
            .expect("the warnings")
            .iter()
            .filter(|warning| warning["what"] == "press_point_only")
            .map(|warning| warning["step"].clone())
            .collect::<Vec<_>>(),
        [json!(1)],
        "{ready}"
    );

    let generated = bench.stopped();
    assert_eq!(generated["lab_status"], "generated", "{generated}");
    assert_eq!(
        generated["key_steps"],
        json!([{"step": 3, "op": "step_03_key", "key": BACK, "guard": "ui/detail",
                "replaced_click": true}]),
        "{generated}"
    );
    let first = entries(&generated);
    assert_eq!(
        operation_ids(&task(&first)),
        ["step_01_click", "step_02_select", "step_03_key"],
        "{generated}"
    );
    let cases = &generated["cases"];
    assert_eq!(
        cases["counts"],
        json!({"steps": 3, "final": 1, "endings": 0}),
        "{cases}"
    );
    assert_eq!(
        cases["ids"],
        json!(["step_01", "step_02", "step_03", "final"]),
        "{cases}"
    );
    assert_eq!(cases["not_proven"], json!([]), "{cases}");
    let directory = PathBuf::from(cases["directory"].as_str().expect("the cases' directory"));
    let package = bench.package(&generated);
    for (id, source) in [
        ("step_01", Some(1)),
        ("step_02", Some(4)),
        ("step_03", Some(5)),
        ("final", None),
    ] {
        let case_json = fs::read(directory.join(id).join("case.json")).expect("case.json");
        let scene = fs::read(directory.join(id).join("scene.png")).expect("scene.png");
        if let Some(step) = source {
            let case: Value = serde_json::from_slice(&case_json).expect("case JSON");
            assert_eq!(
                case["source"],
                json!({"record_id": RECORD_ID, "step": step}),
                "{case}"
            );
        }
        if id == "step_02" {
            // The start mark's noise of the list at offset 0, where the press screen at offset 48
            // shows the list's gray.
            assert_eq!(
                scene_pixel(&scene, 64, 21),
                noise(104, 101),
                "the list's first screen"
            );
        }
        let verdict = run_regression_case(&package, &case_json, &scene, &bench.ocr);
        assert_eq!(
            verdict.verdict,
            RegressionVerdict::Passed,
            "{id}: {verdict:?}"
        );
    }

    bench.header("stopped");
    let unchanged = bench.marked(json!({"step": 1, "key": false}));
    assert!(unchanged.after_stop, "a mark after the stop");
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

// The train's rule 6: the joined recording archived: the manifest's material names the press of
// every step 1 to 5, the folded swipes kept as recorded (each with the frame it was sent on and
// where it began and ended), the key step's entry of the kind `key` with the frame, the point and
// the outcome of the press it replaced, and the final screen, step 6's; its rows are the six
// steps, each of its kind, the key step's with the frames of the press it replaced; the step table
// keeps its bytes.
#[test]
fn t70_the_archive_keeps_the_folded_swipes_and_the_key_steps_row() {
    let bench = joined();
    let table = bench.recording_bytes();
    let recording = bench.recording();
    let archived = record_archive(
        &bench.lock,
        &RecordArchiveOptions {
            written_by: "gate_463_t70".to_owned(),
            ..RecordArchiveOptions::default()
        },
    )
    .unwrap_or_else(|error| panic!("record archive: {}: {:?}", error.code, error.details));
    assert_eq!(archived.status, "archived");
    let manifest: Value =
        serde_json::from_slice(&fs::read(Path::new(&archived.manifest.path)).expect("manifest"))
            .expect("manifest JSON");
    let material = &manifest["material"];
    assert_eq!(
        material["clicks"]
            .as_array()
            .expect("the material's presses")
            .iter()
            .map(|click| (click["step"].clone(), click["kind"].clone()))
            .collect::<Vec<_>>(),
        [
            (json!(1), json!("tap")),
            (json!(2), json!("swipe")),
            (json!(3), json!("swipe")),
            (json!(4), json!("tap")),
            (json!(5), json!("key"))
        ],
        "{material}"
    );
    for swipe in [2, 3] {
        let click = row(&material["clicks"], "step", swipe);
        assert_eq!(
            click["frame"]["frame_id"],
            step_of(&recording, swipe)["click"]["execution"]["before_frame"],
            "{click}"
        );
        assert_eq!(click["point"], json!(point(FORWARD.0)), "{click}");
        assert_eq!(click["to"], json!(point(FORWARD.1)), "{click}");
    }
    let replaced = &step_of(&recording, 5)["key"]["replaced"]["execution"];
    let keyed = row(&material["clicks"], "step", 5);
    assert_eq!(
        keyed["frame"]["frame_id"], replaced["before_frame"],
        "{keyed}"
    );
    assert_eq!(keyed["point"], json!(point(KEY_POINT)), "{keyed}");
    assert_eq!(keyed["outcome"], "performed", "{keyed}");
    assert_eq!(material["final_frame"]["step"], 6, "{material}");

    let rows = &manifest["steps"];
    assert_eq!(rows.as_array().map(Vec::len), Some(6), "{manifest}");
    for (step, kind) in [
        (1, "tap"),
        (2, "swipe"),
        (3, "swipe"),
        (4, "tap"),
        (5, "key"),
        (6, "none"),
    ] {
        assert_eq!(row(rows, "step", step)["kind"], kind, "{manifest}");
    }
    let key_row = row(rows, "step", 5);
    assert_eq!(
        key_row["before_frame"], replaced["before_frame"],
        "{key_row}"
    );
    assert_eq!(
        key_row["after_frame"]["frame_id"], replaced["after_frame"],
        "{key_row}"
    );
    assert_eq!(key_row["outcome"], "performed", "{key_row}");
    assert_eq!(
        bench.recording_bytes(),
        table,
        "the step table keeps its bytes"
    );
}
