// SPDX-License-Identifier: AGPL-3.0-only

// #463 S2-9a (contracts/lab-recording.md, "A scrolling choice: reading the list to its end";
// register §1.457): a press step whose recorded swipes are folded into it (S2-8') declares the
// first kind of scrolling choice: `select` with `rows` and `scan {list, start, end, stop: "end"}`.
// Recordings are made as stage one makes them (taps and swipes through `record_press` with a fake
// Runtime, its captures taking the scripted screens and the caller's clock moving only by its
// waits). Frames are synthetic 128x128 screens: a page square of one colour at (0, 0, 16, 16) and,
// on the list screens, a list area at rows 20 to 99 that shows the content rows `offset` to
// `offset + 79`. The content holds eleven rows 12 px apart (row k at content row 4 + 12k), each an
// anchor of noise (8x6 at x 4) and a name marker pixel at (20, top + 2) whose grey value the
// stand-in name reader reads as the row's name; a start mark of noise at content rows 0 to 3 and an
// end mark of other noise at content rows 136 to 141, both at x 60 to 75. The demonstration: a tap
// on the start screen opens the list at offset 0 (step 1), three swipes forward of 24 px each
// (steps 2 to 4: offsets 24, 48, 72, the list's end), two swipes back (steps 5 and 6: 48, 24), the
// press on the row `delta` (step 7), the detail screen (step 8). No Runtime, no device, no OCR model
// but the stand-in reader; the tests read no clock and never sleep. Every expected number comes
// from the work order's rules and the geometry above, never from the code.
// - `s29a_three_forward_two_back_and_the_press_are_marked_generated_and_admitted` (1)
// - `s29a_a_name_the_list_does_not_hold_decides_empty_over_complete_coverage` (2)
// - `s29a_each_refusal_of_the_request_and_of_the_self_test_writes_nothing` (3)
// - `s29a_a_given_number_replaces_the_measured_one_and_is_marked_given` (4)
// - `s29a_a_select_without_the_list_end_is_listed_and_refuses_record_stop` (5)
// - `s29a_the_package_is_generated_byte_for_byte_again` (6)
// - `s29a_unfold_is_refused_while_the_select_stands_and_clearing_it_keeps_the_fold` (7)
// - `s29a_the_regression_case_of_the_step_is_on_its_first_screen_and_passes` (8)
// Fix round 1 (the independent review; the coordinator's rulings):
// - `s29a_fix1_a_pressed_name_the_press_screen_does_not_show_is_refused`
// - `s29a_fix1_an_end_mark_that_passes_and_then_not_is_lost`
// - `s29a_fix1_an_end_mark_that_passes_before_the_last_forward_screen_is_refused`
// - `s29a_fix1_a_first_swipe_longer_than_the_kernel_admits_has_its_own_word`
// - `s29a_fix1_a_folded_swipe_without_its_screen_after_is_refused`
// - `s29a_fix1_a_scrolling_select_on_the_first_step_decides_first_over_its_screens`

use crate::recording::generate::{
    DEFAULT_APPLICATION_ARRIVAL_TIMEOUT_MS, DEFAULT_ARRIVAL_TIMEOUT_MS, StopSettings,
    plan as generate_plan,
};
use crate::recording::model::LabRecording;
use crate::recording::store::{lab_dir, old_record_path, recording_path};
use crate::recording_api::package_without_cases;
use crate::{
    AfterCapture, AttachFrameRequest, CapturedFrame, ClickEffect, ClickPlan, CommitClickRequest,
    LAB_RECORD_MARK_SCHEMA, LabCode, LabOcr, MarkOutcome, MarkRequest, PlanClickRequest,
    PlannedSwipe, PointPress, RecordFramePorts, RecordPoint, RecordPressRequest, RecordRect,
    RecordReviewRequest, RecordStartOptions, RecordStopOptions, RecordingLock, record_mark,
    record_press, record_ready, record_review, record_start, record_stop,
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
const RECORD_ID: &str = "record_0463_s29a";
const TASK_ID: &str = "neutral_task";
const STARTED_AT: u64 = 1_700_000_000_000;
const WIDTH: u32 = 128;
const HEIGHT: u32 = 128;
const GRAY: [u8; 3] = [60, 60, 60];
const RED: [u8; 3] = [230, 20, 20];
const GREEN: [u8; 3] = [20, 230, 20];
const YELLOW: [u8; 3] = [230, 230, 20];
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

const fn rect(x: i32, y: i32, width: i32, height: i32) -> RecordRect {
    RecordRect {
        x,
        y,
        width,
        height,
    }
}

/// Each screen's page square: red on the start screen, green on the list, yellow on the detail.
const SQUARE: RecordRect = rect(0, 0, 16, 16);
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
/// The grey value of row k's name marker is 200 + k; this one no name maps to.
const UNREADABLE: u8 = 190;
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
/// The end mark on the last screen after a forward swipe (offset 72): content row 136 at 84.
const END_MARK: RecordRect = rect(60, 84, 16, 6);

/// The tap on the start screen, and the press on the row `delta` (row 3, at frame rows 36 to 43 on
/// the screen at offset 24, where the press is made).
const START_POINT: (i32, i32) = (100, 110);
const ROW_POINT: (i32, i32) = (10, 40);
/// The pressed row's box on that screen: the row's rectangle, moved with the list.
const PRESSED_ROW: RecordRect = rect(2, 36, 40, 8);

/// A swipe by its two points: up the list by 24 px, and back down by as much.
type Swipe = ((i32, i32), (i32, i32));
const FORWARD: Swipe = ((64, 80), (64, 56));
const BACK: Swipe = ((64, 56), (64, 80));

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

// The list scrolled by `offset` px, row `unreadable` (if any) with a name marker no name maps to.
fn list_at(offset: u32, unreadable: Option<u32>) -> Vec<u8> {
    list_drawn(offset, unreadable, None)
}

// Fix round 1: the list scrolled by `offset` px with a copy of the end mark's noise at the
// content rows `copy` to `copy + 5` too, at the end mark's columns.
fn list_copying_end(offset: u32, copy: u32) -> Vec<u8> {
    list_drawn(offset, None, Some(copy))
}

fn list_drawn(offset: u32, unreadable: Option<u32>, end_copy: Option<u32>) -> Vec<u8> {
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
            if let Some(copy) = end_copy
                && (copy..copy + END_ROWS).contains(&row)
            {
                return noise(x - MARK_X + 150, row - copy + 150);
            }
        }
        for k in 0..ROWS {
            let top = FIRST_ROW + PITCH * k;
            if (top..top + 6).contains(&row) && (4..12).contains(&x) {
                return noise(x - 4, row - top);
            }
            if row == top + 2 && x == 20 {
                let value = if unreadable == Some(k) {
                    UNREADABLE
                } else {
                    200 + u8::try_from(k).expect("a row")
                };
                return [value; 3];
            }
        }
        GRAY
    })
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

// The scrolling select of the first kind over the list, the wanted value's default `delta`, the
// row the demonstration pressed.
fn scrolling() -> Value {
    json!({
        "rows": {"row": ROW, "anchor": ANCHOR, "name": NAME},
        "scan": {"list": LIST, "start": START_MARK, "end": END_MARK, "stop": "end"},
        "read": {"languages": ["en"], "timeout_ms": 1000, "minimum_confidence_milli": 800,
                 "model_ref": "neutral-ocr", "model_sha256": MODEL_SHA256},
        "names": NAMES
            .iter()
            .zip(ALIASES)
            .map(|(id, alias)| json!({"id": id, "aliases": [alias]}))
            .collect::<Vec<_>>(),
        "match": {"max_distance": 1, "minimum_margin": 1},
        "wanted": {"name": "list_item", "default": "delta"}
    })
}

// The mark request of the scrolling select `select` on the press step 7.
fn on_press(select: Value) -> Value {
    json!({"step": 7, "select": select})
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

// The screen after a press as one capture 800 ms after it, without a stillness test: the after
// loop records the interval 100 and the wait 800.
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

fn task_of(entries: &BTreeMap<String, Vec<u8>>) -> Value {
    serde_json::from_slice(
        entries
            .get(&format!("resources/operations/{TASK_ID}/task.json"))
            .expect("task.json"),
    )
    .expect("task JSON")
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

// The select's refusal of `record_mark_rejected` with `reason`.
fn rejection(error: LabError, reason: &str) -> Value {
    assert_eq!(
        error.code, "record_mark_rejected",
        "{}: {:?}",
        error.message, error.details
    );
    let details = error.details.expect("details");
    details["marks"]
        .as_array()
        .and_then(|marks| {
            marks
                .iter()
                .find(|mark| mark["id"] == "select" && mark["reason"] == reason)
        })
        .cloned()
        .unwrap_or_else(|| panic!("no select {reason} rejection in {details}"))
}

// The row of `step` among `rows`, by the member `key` that holds its serial number.
fn row<'v>(rows: &'v Value, key: &str, step: u32) -> &'v Value {
    rows.as_array()
        .unwrap_or_else(|| panic!("no rows in {rows}"))
        .iter()
        .find(|item| item[key] == step)
        .unwrap_or_else(|| panic!("no step {step} in {rows}"))
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
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_463_s29a", "Lab-CLI")
            .expect("recording lock");
        let bench = Self {
            lock,
            ocr: LabOcr::with_provider(Arc::new(RowNames)),
            temp,
        };
        fs::write(
            old_record_path(&bench.state(), INSTANCE),
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

    fn recording(&self) -> Value {
        serde_json::from_slice(
            &fs::read(recording_path(&self.state(), RECORD_ID)).expect("step table"),
        )
        .expect("step table JSON")
    }

    fn files(&self) -> Vec<(String, Vec<u8>)> {
        files_under(&lab_dir(&self.state(), RECORD_ID))
    }

    // `record review --step <k> --no-preview`.
    fn review(&self, step: u32) -> Value {
        let asked = RecordReviewRequest {
            record_id: None,
            step: Some(step),
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

    // `record stop` (with `dry_run`): the `lab` output with its status.
    fn stop(&self, dry_run: bool) -> Result<Value, LabError> {
        let options = RecordStopOptions {
            dry_run,
            ..RecordStopOptions::default()
        };
        record_stop(&self.lock, &options, &self.ocr).map(|outcome| {
            let mut lab = serde_json::to_value(&outcome.lab).expect("lab JSON");
            lab["lab_status"] = json!(outcome.lab_status);
            lab
        })
    }

    fn stopped(&self) -> Value {
        self.stop(false).unwrap_or_else(|error| {
            panic!(
                "record stop: {}: {} {:?}",
                error.code, error.message, error.details
            )
        })
    }
}

// The demonstration with row `unreadable` (if any) unreadable on every screen: steps 1 (the tap),
// 2 to 4 (forward), 5 and 6 (back), 7 (the press on `delta`), 8 (the detail screen); each page
// marked (steps 1, 7 and 8), the swipes not yet folded.
fn demonstrated(unreadable: Option<u32>) -> Bench {
    demonstrated_on(&|offset| list_at(offset, unreadable))
}

// Fix round 1: the same demonstration on the list screens `screen` draws at each offset.
fn demonstrated_on(screen: &dyn Fn(u32) -> Vec<u8>) -> Bench {
    let bench = Bench::new();
    bench.press_at(START_POINT, [start(), screen(0)]);
    for (swipe, (from, to)) in [FORWARD, FORWARD, FORWARD, BACK, BACK].into_iter().zip([
        (0, 24),
        (24, 48),
        (48, 72),
        (72, 48),
        (48, 24),
    ]) {
        bench.drag_list(swipe, [screen(from), screen(to)]);
    }
    bench.press_at(ROW_POINT, [screen(24), detail()]);
    bench.marked(json!({"step": 1, "add": [color("ui/start", SQUARE)]}));
    bench.marked(json!({"step": 7, "add": [color("ui/list", SQUARE)]}));
    bench.marked(json!({"step": 8, "add": [color("ui/detail", SQUARE)]}));
    bench
}

// The demonstration with its five swipes folded into the press step 7.
fn folded(unreadable: Option<u32>) -> Bench {
    let bench = demonstrated(unreadable);
    bench.marked(json!({"step_action": {"kind": "fold_scan", "step": 7, "first": 2}}));
    bench
}

// Fix round 1: the demonstration on the screens `screen` draws, its swipes folded into step 7.
fn folded_on(screen: &dyn Fn(u32) -> Vec<u8>) -> Bench {
    let bench = demonstrated_on(screen);
    bench.marked(json!({"step_action": {"kind": "fold_scan", "step": 7, "first": 2}}));
    bench
}

// One number as the select's scan holds it, labelled `origin` (fix round 1: what it is).
fn labelled(name: &str, value: u64, origin: &str, from: &str) -> Value {
    json!({"name": name, "value": value, "origin": origin, "from": from})
}

// A number read off the agent's regions or the demonstration.
fn measured(name: &str, value: u64, from: &str) -> Value {
    labelled(name, value, "measured", from)
}

// A number computed from measured ones by a stated formula.
fn derived(name: &str, value: u64, from: &str) -> Value {
    labelled(name, value, "derived", from)
}

// Every number the select writes on this demonstration, in order, by the work order's rules and
// the geometry, each labelled by what it is (fix round 1, the coordinator's ruling): the list 80 px
// and the row 8 px along the axis (window 1, a constant, to 72 = 80 - 8, derived); the first
// forward swipe 300 ms with its after loop's interval 100 (measured); every swipe's screen captured
// 800 ms after it (settle 2 x 800 + 100, derived); tolerance max(2, 8 / 10) = 2 and the edge margin
// (derived); the row 40 x 8, 5 cells of it, one across (derived); the Runtime's frame-change bound
// 1500 and one overlapping row (constants); one row across (measured); rows 12 apart and the first 4
// below the list's top (12 + 2); the last row on the screen at offset 72 starting at frame row 72,
// 28 above the list's end (28 + 2); the rows 2 px from the list's left edge (2 + 2), no gap across
// (0 + 2); six anchors on a screen at most (6 + 2); four forward screens (8 viewports, 8 return
// viewports); the scan's time (8 + 8 + 1) x (300 + 1700) + 8 x 1000 (all derived).
fn every_number() -> Value {
    json!([
        labelled("advance_min_content_step_px", 1, "default", "generator"),
        derived("advance_max_content_step_px", 72, "list_and_row"),
        measured("advance_duration_ms", 300, "first_forward_swipe"),
        derived("reposition_max_content_step_px", 72, "list_and_row"),
        measured("settle_interval_ms", 100, "first_forward_swipe"),
        derived("settle_timeout_ms", 1700, "fold_swipes"),
        derived("position_tolerance_px", 2, "row"),
        derived("edge_margin_px", 2, "row"),
        derived("signature_columns", 5, "row"),
        derived("signature_rows", 1, "row"),
        labelled(
            "signature_max_distance_milli",
            1500,
            "default",
            "frame_change_bound",
        ),
        labelled("min_overlap_instances", 1, "default", "generator"),
        measured("spacing_columns", 1, "screens"),
        derived("spacing_max_axis_gap_px", 14, "screens"),
        derived("spacing_max_trailing_gap_px", 30, "screens"),
        derived("spacing_max_cross_lead_px", 4, "screens"),
        derived("spacing_max_cross_gap_px", 2, "screens"),
        derived("max_instances", 8, "screens"),
        derived("max_viewports", 8, "fold_swipes"),
        derived("max_reposition_viewports", 8, "fold_swipes"),
        derived("max_ms", 42_000, "scan_moves"),
    ])
}

// The names each screen shows, in order: offsets 0, 24, 48 and 72 forward, then 48 and 24 back.
fn names_on_screens() -> Vec<Value> {
    [0_usize, 2, 4, 6, 4, 2]
        .into_iter()
        .zip([6_usize, 6, 6, 5, 6, 6])
        .map(|(first, count)| json!(NAMES[first..first + count]))
        .collect()
}

// The work order's test 1: the demonstration "three forward, two back, press" folded and marked:
// the select stands, its click the pressed row's box, every number measured by the rules above; the
// offline entry decides `delta` (position 3) over complete coverage, each forward swipe a step of
// 24 px, and `lima` empty; record ready lists nothing; record stop generates the select over a scan
// (its layout, its scan, its targets), the kernel admits it, and lab.selects carries the offline
// entry's decision; record review lists the screens, the names read and every number.
#[test]
fn s29a_three_forward_two_back_and_the_press_are_marked_generated_and_admitted() {
    let bench = folded(None);
    let marked = bench.marked(on_press(scrolling()));
    let click = serde_json::to_value(&marked.click).expect("click JSON");
    assert_eq!(click["source"], "select", "{click}");
    assert_eq!(click["rect"], json!(PRESSED_ROW), "{click}");
    let select = serde_json::to_value(&marked.select).expect("select JSON");
    assert_eq!(select["self_test"]["status"], "passed", "{select}");
    assert_eq!(select["scan"]["numbers"], every_number(), "{select}");
    assert_eq!(select["scan"]["axis"], "vertical", "{select}");
    assert_eq!(select["scan"]["advance_from"], json!({"x": 64, "y": 80}));
    assert_eq!(select["scan"]["advance_to"], json!({"x": 64, "y": 56}));
    let outcome = &select["scan"]["outcome"];
    assert_eq!(
        outcome,
        &json!({
            "coverage": "complete",
            "positions": 11,
            "pressed": {"result": "selected", "name": "delta", "position": 3},
            "steps": [{"step": 2, "content_step_px": 24}, {"step": 3, "content_step_px": 24},
                      {"step": 4, "content_step_px": 24}],
            "absent": {"name": "lima", "coverage": "complete", "decision": {"result": "empty"}}
        }),
        "{select}"
    );
    let screens = select["scan"]["screens"].as_array().expect("the screens");
    assert_eq!(
        screens
            .iter()
            .map(|screen| (
                screen["step"].clone(),
                screen["role"].clone(),
                screen["content_offset_px"].clone()
            ))
            .collect::<Vec<_>>(),
        [
            (json!(2), json!("first"), json!(0)),
            (json!(2), json!("forward"), json!(24)),
            (json!(3), json!("forward"), json!(48)),
            (json!(4), json!("forward"), json!(72)),
            (json!(5), json!("back"), Value::Null),
            (json!(6), json!("back"), Value::Null)
        ],
        "{select}"
    );
    assert_eq!(
        screens
            .iter()
            .map(|screen| screen["names"].clone())
            .collect::<Vec<_>>(),
        names_on_screens()
    );
    assert_eq!(
        screens[0]["regions"],
        json!(["list", "row", "anchor", "name", "start"])
    );
    assert_eq!(screens[3]["regions"], json!(["list", "end"]));

    let ready = bench.ready();
    assert_eq!(ready["missing"], json!([]), "{ready}");
    assert_eq!(ready["status"], "ready", "{ready}");

    let lab = bench.stopped();
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let entries = package(&lab);
    let task = task_of(&entries);
    let operation = task["operations"]
        .as_array()
        .and_then(|operations| {
            operations
                .iter()
                .find(|operation| operation["id"] == "step_02_select")
        })
        .expect("the select operation");
    let scan = &operation["select"]["scan"];
    assert_eq!(
        scan["advance"],
        json!({"kind": "drag", "from": {"x": 64, "y": 80, "width": 1, "height": 1},
               "to": {"x": 64, "y": 56, "width": 1, "height": 1}, "duration_ms": 300,
               "min_content_step_px": 1, "max_content_step_px": 72}),
        "{operation}"
    );
    assert_eq!(
        scan["start"],
        json!({"check": "select/step_02/start", "rewind": true})
    );
    assert_eq!(scan["end"], json!({"check": "select/step_02/end"}));
    assert_eq!(
        scan["bounds"],
        json!({"max_viewports": 8, "max_reposition_viewports": 8, "max_ms": 42_000})
    );
    assert_eq!(scan.get("early_stop"), None, "{scan}");
    assert_eq!(operation["select"].get("confirm"), None, "{operation}");
    let layout = &task["candidate_layouts"][0];
    assert_eq!(layout["kind"], "repeated_anchor", "{layout}");
    assert_eq!(layout["anchor"], "select/step_02/anchor", "{layout}");
    assert_eq!(
        layout["instance_rect"],
        json!({"x": -2, "y": 0, "width": 40, "height": 8})
    );
    assert_eq!(layout["readable_band"], json!(LIST));
    assert_eq!(layout["features"][0]["offset"], json!({"x": 14, "y": 0}));
    assert!(
        entries.contains_key(&format!(
            "resources/operations/{TASK_ID}/assets/select/step_02_anchor.png"
        )),
        "{:?}",
        entries.keys().collect::<Vec<_>>()
    );
    let checked = &lab["selects"][0];
    assert_eq!(checked["op"], "step_02_select", "{checked}");
    assert_eq!(checked["scan"]["coverage"], "complete", "{checked}");
    assert_eq!(
        checked["scan"]["pressed"],
        json!({"result": "selected", "name": "delta", "position": 3}),
        "{checked}"
    );

    let review = bench.review(7);
    let sheet = &review["steps"][0];
    assert_eq!(sheet["scan"]["numbers"], every_number(), "{sheet}");
    assert_eq!(sheet["scan"]["outcome"], *outcome, "{sheet}");
    assert_eq!(
        sheet["scan"]["screens"]
            .as_array()
            .expect("the sheet's screens")
            .iter()
            .map(|screen| screen["names"].clone())
            .collect::<Vec<_>>(),
        names_on_screens()
    );
}

// The work order's test 2: `lima`, a name the list does not hold, decides empty over complete
// coverage at mark time and in record stop's self-check; so does the wanted value's default when it
// is `lima` (the demonstration still pressing `delta`).
#[test]
fn s29a_a_name_the_list_does_not_hold_decides_empty_over_complete_coverage() {
    let bench = folded(None);
    let mut select = scrolling();
    select["wanted"]["default"] = json!("lima");
    select["pressed"] = json!("delta");
    let marked = bench.marked(on_press(select));
    let scan = serde_json::to_value(&marked.select).expect("select JSON")["scan"].clone();
    assert_eq!(
        scan["outcome"]["absent"],
        json!({"name": "lima", "coverage": "complete", "decision": {"result": "empty"}}),
        "{scan}"
    );
    assert_eq!(
        scan["outcome"]["pressed"],
        json!({"result": "selected", "name": "delta", "position": 3}),
        "{scan}"
    );
    let lab = bench.stopped();
    assert_eq!(
        lab["selects"][0]["scan"]["absent"],
        json!({"name": "lima", "coverage": "complete", "decision": {"result": "empty"}}),
        "{lab}"
    );
}

// The work order's test 3: each refusal of the request (item 1) and of the self-test (item 3) that a
// recorded demonstration can show, nothing written each time.
#[test]
fn s29a_each_refusal_of_the_request_and_of_the_self_test_writes_nothing() {
    let unfolded = demonstrated(None);
    refusal(
        unfolded.refused(on_press(scrolling())),
        LabCode::RecordSelectInvalid.as_str(),
        "select_problem",
        "scan_without_fold",
    );
    let bench = folded(None);
    let with = |member: &str, value: Value| {
        let mut select = scrolling();
        select[member] = value;
        on_press(select)
    };
    let scanned = |member: &str, value: Value| {
        let mut select = scrolling();
        select["scan"][member] = value;
        on_press(select)
    };
    let rowed = |member: &str, value: Value| {
        let mut select = scrolling();
        select["rows"][member] = value;
        on_press(select)
    };
    let mut stopless = scrolling();
    stopless["scan"]
        .as_object_mut()
        .expect("the scan")
        .remove("stop");
    for (value, word) in [
        (
            with("slots", json!([{"rect": ROW, "name": NAME}])),
            "rows_with_slots",
        ),
        (with("confirm", json!("decision")), "confirm_with_rows"),
        (on_press(stopless), "scan_stop_missing"),
        (
            scanned("list", json!(rect(0, 20, 129, 80))),
            "scan_region_outside_frame",
        ),
        (
            rowed("row", json!(rect(2, 10, 40, 8))),
            "scan_region_outside_list",
        ),
        (
            rowed("name", json!(rect(18, 24, 30, 6))),
            "scan_region_outside_row",
        ),
    ] {
        refusal(
            bench.refused(value),
            LabCode::RecordSelectInvalid.as_str(),
            "select_problem",
            word,
        );
    }
    // The pressed name `echo` (row 4, at frame rows 48 to 55) is not the row the press hit.
    let detail = rejection(
        bench.refused(with("pressed", json!("echo"))),
        "select_press_mismatch",
    );
    assert_eq!(detail["point"], json!(point(ROW_POINT)), "{detail}");
    // A start mark on the page square's corner passes on every screen of the list.
    rejection(
        bench.refused(scanned("start", json!(rect(8, 8, 16, 8)))),
        "scan_start_mark_repeated",
    );
    // A window whose smallest step is 30 px holds no swipe of 24 px.
    let detail = rejection(
        bench.refused(scanned(
            "numbers",
            json!({"advance_min_content_step_px": 30}),
        )),
        "scan_step_outside_window",
    );
    assert_eq!(detail["content_step_px"], 24, "{detail}");
    assert_eq!(detail["window"], json!([30, 72]), "{detail}");
    // A row whose name reads no known name on the screens that show it.
    let unread = folded(Some(8));
    let detail = rejection(unread.refused(on_press(scrolling())), "select_name_unread");
    assert_eq!(detail["cause"], "low_confidence", "{detail}");
}

// The work order's test 4: `settle_timeout_ms` given as 2500 replaces the measured 1700, is marked
// given with the measured value beside it, and is the package's; the scan's time follows from it,
// (8 + 8 + 1) x (300 + 2500) + 8 x 1000.
#[test]
fn s29a_a_given_number_replaces_the_measured_one_and_is_marked_given() {
    let bench = folded(None);
    let mut select = scrolling();
    select["scan"]["numbers"] = json!({"settle_timeout_ms": 2500});
    let marked = bench.marked(on_press(select));
    let numbers =
        serde_json::to_value(&marked.select).expect("select JSON")["scan"]["numbers"].clone();
    let entry = |name: &str| {
        numbers
            .as_array()
            .and_then(|numbers| numbers.iter().find(|number| number["name"] == name))
            .cloned()
            .unwrap_or_else(|| panic!("no {name} in {numbers}"))
    };
    assert_eq!(
        entry("settle_timeout_ms"),
        json!({"name": "settle_timeout_ms", "value": 2500, "origin": "given",
               "from": "fold_swipes", "measured": 1700})
    );
    assert_eq!(entry("max_ms"), derived("max_ms", 55_600, "scan_moves"));
    let lab = bench.stopped();
    let task = task_of(&package(&lab));
    let operation = task["operations"]
        .as_array()
        .and_then(|operations| {
            operations
                .iter()
                .find(|operation| operation["id"] == "step_02_select")
        })
        .expect("the select operation");
    assert_eq!(
        operation["select"]["scan"]["settle"],
        json!({"interval_ms": 100, "timeout_ms": 2500})
    );
}

// The work order's test 5: a select declared without the end mark (the list's end never recorded)
// stands; record ready lists `scan_end_not_recorded` on the press step, its one item; record stop
// and its dry run refuse with `end_not_recorded`, nothing written.
#[test]
fn s29a_a_select_without_the_list_end_is_listed_and_refuses_record_stop() {
    let bench = folded(None);
    let mut select = scrolling();
    select["scan"]
        .as_object_mut()
        .expect("the scan")
        .remove("end");
    let marked = bench.marked(on_press(select));
    let scan = serde_json::to_value(&marked.select).expect("select JSON")["scan"].clone();
    assert_eq!(scan.get("outcome"), None, "{scan}");
    let ready = bench.ready();
    assert_eq!(ready["missing_total"], 1, "{ready}");
    let item = &ready["missing"][0];
    assert_eq!(item["what"], "scan_end_not_recorded", "{item}");
    assert_eq!(item["code"], LabCode::RecordScanInvalid.as_str(), "{item}");
    assert_eq!(item["step"], 7, "{item}");
    assert_eq!(item["artifact_step"], 2, "{item}");
    let before = bench.files();
    for dry_run in [false, true] {
        let details = refusal(
            bench
                .stop(dry_run)
                .expect_err("a select without the list's end"),
            LabCode::RecordScanInvalid.as_str(),
            "scan_problem",
            "end_not_recorded",
        );
        assert_eq!(details["missing_total"], 1, "{details}");
        assert_eq!(bench.files(), before, "dry run {dry_run}: a file changed");
    }
}

// The work order's test 6: the package of a recording with this select is generated byte for byte
// again: built twice from the same recording by the self-checks that build it, and written by record
// stop, the three are one.
#[test]
fn s29a_the_package_is_generated_byte_for_byte_again() {
    let bench = folded(None);
    bench.marked(on_press(scrolling()));
    let recording: LabRecording = serde_json::from_slice(
        &fs::read(recording_path(&bench.state(), RECORD_ID)).expect("step table"),
    )
    .expect("step table JSON");
    let settings = StopSettings {
        game: Some("neutral".to_owned()),
        server: Some("test".to_owned()),
        locale: Some("en-US".to_owned()),
        package_id: None,
        requires: None,
        timeout_ms: None,
        arrival_timeout_ms: DEFAULT_ARRIVAL_TIMEOUT_MS,
        application_arrival_timeout_ms: DEFAULT_APPLICATION_ARRIVAL_TIMEOUT_MS,
    };
    let build = || {
        let plan = generate_plan(&recording, &settings)
            .unwrap_or_else(|error| panic!("the plan: {}: {}", error.code, error.message));
        package_without_cases(&plan, &bench.ocr).expect("the package")
    };
    let first = build();
    let second = build();
    assert!(first == second, "the package was built otherwise again");
    let lab = bench.stopped();
    let written = fs::read(lab["path"].as_str().expect("package path")).expect("package");
    assert!(written == first, "record stop wrote another package");
}

// The work order's test 7: unfold_scan of the press step is refused (`scan_selected`) while the
// select stands, nothing written; clear_select takes the select off, gives the press back its own
// point, keeps the fold, and record ready lists `scan_without_select` again.
#[test]
fn s29a_unfold_is_refused_while_the_select_stands_and_clearing_it_keeps_the_fold() {
    let bench = folded(None);
    bench.marked(on_press(scrolling()));
    refusal(
        bench.refused(json!({"step_action": {"kind": "unfold_scan", "step": 7}})),
        LabCode::RecordScanInvalid.as_str(),
        "scan_problem",
        "scan_selected",
    );
    let fold = bench.recording()["steps"]
        .as_array()
        .and_then(|steps| steps.iter().find(|step| step["index"] == 7))
        .expect("the press step")["scan_fold"]
        .clone();
    bench.marked(json!({"step": 7, "clear_select": true}));
    let recorded = bench.recording();
    let press = row(&recorded["steps"], "index", 7);
    assert_eq!(press.get("select"), None, "{press}");
    assert_eq!(press["scan_fold"], fold, "{press}");
    assert_eq!(press["click"]["source"], "point", "{press}");
    let ready = bench.ready();
    assert_eq!(
        ready["missing"]
            .as_array()
            .expect("missing")
            .iter()
            .map(|item| (item["what"].clone(), item["step"].clone()))
            .collect::<Vec<_>>(),
        [(json!("scan_without_select"), json!(7))],
        "{ready}"
    );
}

// The work order's test 8: record stop writes a case per step and the final screen; the select
// step's case is on the list's first screen (the first swipe's screen before it), expects its page,
// and passes, as every case does.
#[test]
fn s29a_the_regression_case_of_the_step_is_on_its_first_screen_and_passes() {
    let bench = folded(None);
    bench.marked(on_press(scrolling()));
    let first_screen = bench.recording()["steps"]
        .as_array()
        .and_then(|steps| steps.iter().find(|step| step["index"] == 2))
        .expect("the first swipe")["click"]["execution"]["before_frame"]
        .clone();
    let lab = bench.stopped();
    assert_eq!(
        lab["cases"]["ids"],
        json!(["step_01", "step_02", "final"]),
        "{lab}"
    );
    assert_eq!(lab["cases"]["not_proven"], json!([]), "{lab}");
    assert_eq!(lab["cases"]["warnings"], json!([]), "{lab}");
    let directory = PathBuf::from(lab["cases"]["directory"].as_str().expect("cases directory"));
    let case: Value = serde_json::from_slice(
        &fs::read(directory.join("step_02").join("case.json")).expect("the step's case"),
    )
    .expect("case JSON");
    assert_eq!(case["source"], json!({"record_id": RECORD_ID, "step": 7}));
    assert_eq!(
        case["expect"]["observe"]["page"], "neutral/step_02",
        "{case}"
    );
    let select = row(&bench.recording()["steps"], "index", 7)["select"]["scan"]["screens"][0]
        ["frame"]["frame_id"]
        .clone();
    assert_eq!(select, first_screen);
}

// Fix round 1: the frame stored as the screen after the swipe `step`.
fn screen_after(bench: &Bench, step: u32) -> Value {
    row(&bench.recording()["steps"], "index", step)["click"]["execution"]["after_frame"].clone()
}

// Fix round 1 (the review's required point 2): the pressed name `alpha` (row 0) is on no row of the
// press step's frame, the screen at offset 24, which shows rows 2 to 7: the self-test refuses
// `select_pressed_absent`, naming the pressed name, nothing written.
#[test]
fn s29a_fix1_a_pressed_name_the_press_screen_does_not_show_is_refused() {
    let bench = folded(None);
    let mut select = scrolling();
    select["pressed"] = json!("alpha");
    let detail = rejection(bench.refused(on_press(select)), "select_pressed_absent");
    assert_eq!(detail["pressed"], "alpha", "{detail}");
}

// Fix round 1 (the review's required point 2, its answer 8): a copy of the end mark at content rows
// 88 to 93 shows in the end mark's region on the screen at offset 24, not on the one at 48, and the
// list's real end does at 72: the end mark passes, then not, then again: `scan_end_mark_lost` on
// the screen at 48 (swipe 3's screen after), passed on the one at 24 (swipe 2's), nothing written.
#[test]
fn s29a_fix1_an_end_mark_that_passes_and_then_not_is_lost() {
    let bench = folded_on(&|offset| list_copying_end(offset, 88));
    let detail = rejection(bench.refused(on_press(scrolling())), "scan_end_mark_lost");
    assert_eq!(detail["frame_id"], screen_after(&bench, 3), "{detail}");
    assert_eq!(detail["passed_on"], screen_after(&bench, 2), "{detail}");
}

// Fix round 1 (the review's required point 1, its answer 5): a copy of the end mark at content rows
// 112 to 117 shows in the end mark's region on the screen at offset 48 and on every later forward
// screen: the kernel would end its coverage there, before the list's end at 72, so the self-test
// refuses `scan_end_mark_early` on the screen at 48 (swipe 3's screen after), naming the last
// forward screen (swipe 4's), nothing written.
#[test]
fn s29a_fix1_an_end_mark_that_passes_before_the_last_forward_screen_is_refused() {
    let bench = folded_on(&|offset| list_copying_end(offset, 112));
    let detail = rejection(bench.refused(on_press(scrolling())), "scan_end_mark_early");
    assert_eq!(detail["frame_id"], screen_after(&bench, 3), "{detail}");
    assert_eq!(detail["last_frame_id"], screen_after(&bench, 4), "{detail}");
}

// Fix round 1 (the review's answer 1): one forward swipe from the list's top to its end, the finger
// from y 90 to y 18 (72 px), then the press on `kilo` at offset 72: the list's 80 px less the row's
// 8 is 72, so the kernel admits a drag of at most 71 px, and no number cures it:
// `record_select_invalid` with `scan_swipe_too_long`, the swipe, 72 and 71 and the hint, before any
// probe, nothing written.
#[test]
fn s29a_fix1_a_first_swipe_longer_than_the_kernel_admits_has_its_own_word() {
    let bench = Bench::new();
    bench.press_at(START_POINT, [start(), list_at(0, None)]);
    bench.drag_list(((64, 90), (64, 18)), [list_at(0, None), list_at(72, None)]);
    bench.press_at((10, 76), [list_at(72, None), detail()]);
    bench.marked(json!({"step": 1, "add": [color("ui/start", SQUARE)]}));
    bench.marked(json!({"step": 3, "add": [color("ui/list", SQUARE)]}));
    bench.marked(json!({"step": 4, "add": [color("ui/detail", SQUARE)]}));
    bench.marked(json!({"step_action": {"kind": "fold_scan", "step": 3, "first": 2}}));
    let mut select = scrolling();
    select["wanted"]["default"] = json!("kilo");
    let details = refusal(
        bench.refused(json!({"step": 3, "select": select})),
        LabCode::RecordSelectInvalid.as_str(),
        "select_problem",
        "scan_swipe_too_long",
    );
    assert_eq!(details["swipe"], 2, "{details}");
    assert_eq!(details["displacement_px"], 72, "{details}");
    assert_eq!(details["most_px"], 71, "{details}");
    assert!(details["hint"].is_string(), "{details}");
}

// Fix round 1 (the review's answer 6): a swipe recorded without its screen after (as `do --capture
// --record` records one), here the folded swipe 3 with its after frame taken out of the step table
// by hand: the scrolling select cannot read the list's screens, and the mark is refused
// `record_step_frame_missing` on the press step, nothing written.
#[test]
fn s29a_fix1_a_folded_swipe_without_its_screen_after_is_refused() {
    let bench = folded(None);
    let path = recording_path(&bench.state(), RECORD_ID);
    let mut table: Value =
        serde_json::from_slice(&fs::read(&path).expect("step table")).expect("step table JSON");
    let swipe = table["steps"]
        .as_array_mut()
        .expect("steps")
        .iter_mut()
        .find(|entry| entry["index"] == 3)
        .expect("swipe 3");
    swipe["click"]["execution"]["after_frame"] = Value::Null;
    fs::write(
        &path,
        serde_json::to_vec_pretty(&table).expect("step table bytes"),
    )
    .expect("the edited step table");
    let error = bench.refused(on_press(scrolling()));
    assert_eq!(
        error.code, "record_step_frame_missing",
        "{}: {:?}",
        error.message, error.details
    );
    assert_eq!(error.details.expect("details")["step"], 7);
}

// Fix round 1 (the review's answer 6): the swipes open the recording (no tap before them), so the
// press step 6 they fold into (`first` 1) is the package's first step: record stop's self-check
// `first_decision` is the kernel's offline entry over its forward screens with the default values:
// `would_select` of `step_01_select`, `delta` at position 3.
#[test]
fn s29a_fix1_a_scrolling_select_on_the_first_step_decides_first_over_its_screens() {
    let bench = Bench::new();
    for (swipe, (from, to)) in [FORWARD, FORWARD, FORWARD, BACK, BACK].into_iter().zip([
        (0, 24),
        (24, 48),
        (48, 72),
        (72, 48),
        (48, 24),
    ]) {
        bench.drag_list(swipe, [list_at(from, None), list_at(to, None)]);
    }
    bench.press_at(ROW_POINT, [list_at(24, None), detail()]);
    bench.marked(json!({"step": 6, "add": [color("ui/list", SQUARE)]}));
    bench.marked(json!({"step": 7, "add": [color("ui/detail", SQUARE)]}));
    bench.marked(json!({"step_action": {"kind": "fold_scan", "step": 6, "first": 1}}));
    bench.marked(json!({"step": 6, "select": scrolling()}));
    let lab = bench.stopped();
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let decision = &lab["first_decision"];
    assert_eq!(decision["status"], "would_select", "{lab}");
    assert_eq!(decision["operation_label"], "step_01_select", "{lab}");
    assert_eq!(decision["name"], "delta", "{lab}");
    assert_eq!(decision["position"], 3, "{lab}");
}
