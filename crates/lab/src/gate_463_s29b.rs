// SPDX-License-Identifier: AGPL-3.0-only

// #463 S2-9b (contracts/lab-recording.md, "A scrolling choice: straight to the wanted item";
// register §1.458): a press step whose recorded swipes are folded into it (S2-8') declares the
// second kind of scrolling choice: `select` with `rows` and `scan {list, start, end?, stop:
// "wanted"}`, the wanted item known and the list swiped straight to it. Recordings are made as stage
// one makes them (taps and swipes through `record_press` with a fake Runtime, its captures taking
// the scripted screens and the caller's clock moving only by its waits), on S2-9a's synthetic list:
// 128x128 screens, a page square of one colour at (0, 0, 16, 16) and, on the list screens, a list
// area at rows 20 to 99 that shows the content rows `offset` to `offset + 79`; eleven content rows
// 12 px apart (row k at content row 4 + 12k), each an anchor of noise (8x6 at x 4) and a name marker
// pixel at (20, top + 2) whose grey value the stand-in name reader reads as the row's name; a start
// mark of noise at content rows 0 to 3 and an end mark of other noise at content rows 136 to 141,
// both at x 60 to 75. The wanted row is `india` (row 8), which first shows whole on the screen at
// offset 48 (frame rows 72 to 79). Two demonstrations: "two forward swipes, press" (steps 1 the tap
// that opens the list at offset 0, 2 and 3 the swipes to 24 and 48, 4 the press on `india`, 5 the
// detail screen), the list's end never on a screen; and "three forward swipes to the end, one back,
// press" (steps 1, 2 to 4 to offsets 24, 48 and 72, 5 back to 48, 6 the press, 7 the detail
// screen). No Runtime, no device, no OCR model but the stand-in reader; the tests read no clock and
// never sleep. Every expected number comes from the work order's rules and the geometry above.
// - `s29b_two_forward_swipes_and_the_press_without_an_end_stop_early_and_are_admitted` (1)
// - `s29b_with_an_end_given_the_scan_has_both_and_both_offline_runs_are_made` (2)
// - `s29b_an_ending_beside_a_scan_without_an_end_is_refused_at_the_mark` (3)
// - `s29b_a_wanted_default_no_screen_shows_fails_the_self_check` (4)
// - `s29b_record_review_says_the_kind_the_end_and_what_a_run_does_without_the_item` (5)

use crate::recording::store::{lab_dir, old_record_path};
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
const RECORD_ID: &str = "record_0463_s29b";
const TASK_ID: &str = "neutral_task";
const STARTED_AT: u64 = 1_700_000_000_000;
const WIDTH: u32 = 128;
const HEIGHT: u32 = 128;
const GRAY: [u8; 3] = [60, 60, 60];
const RED: [u8; 3] = [230, 20, 20];
const GREEN: [u8; 3] = [20, 230, 20];
const YELLOW: [u8; 3] = [230, 230, 20];
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
/// The key of the run's normal end when the wanted name is in no row, as R1 makes it by default.
const NOT_FOUND_KEY: &str = "neutral_task_not_found";

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
/// The end mark on the screen at offset 72, the last after a forward swipe of the second
/// demonstration: content row 136 at frame row 84.
const END_MARK: RecordRect = rect(60, 84, 16, 6);

/// The tap on the start screen, and the press on the row `india` (row 8, at frame rows 72 to 79 on
/// the screen at offset 48, where the press is made).
const START_POINT: (i32, i32) = (100, 110);
const WANTED_POINT: (i32, i32) = (10, 76);
/// The pressed row's box on that screen: the row's rectangle, moved with the list.
const PRESSED_ROW: RecordRect = rect(2, 72, 40, 8);

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
// `india`, the row the demonstration pressed; with `end`, the end mark declared.
fn straight(end: bool) -> Value {
    let mut scan = json!({"list": LIST, "start": START_MARK, "stop": "wanted"});
    if end {
        scan["end"] = json!(END_MARK);
    }
    json!({
        "rows": {"row": ROW, "anchor": ANCHOR, "name": NAME},
        "scan": scan,
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

// The task document of the package `record stop` wrote.
fn task_of(lab: &Value) -> Value {
    let bytes = fs::read(lab["path"].as_str().expect("package path")).expect("package");
    let container = if lab["container"] == "zip" {
        ContentContainer::Zip
    } else {
        ContentContainer::Json
    };
    let entries: BTreeMap<String, Vec<u8>> =
        expand_content_container(&bytes, container, ContainmentLimits::default())
            .expect("package container");
    serde_json::from_slice(
        entries
            .get(&format!("resources/operations/{TASK_ID}/task.json"))
            .expect("task.json"),
    )
    .expect("task JSON")
}

// The select step's operation of `task`.
fn select_operation(task: &Value) -> Value {
    task["operations"]
        .as_array()
        .and_then(|operations| {
            operations
                .iter()
                .find(|operation| operation["id"] == "step_02_select")
        })
        .cloned()
        .unwrap_or_else(|| panic!("no select operation in {task}"))
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

// The number `name` among `numbers`.
fn number(numbers: &Value, name: &str) -> Value {
    numbers
        .as_array()
        .and_then(|numbers| numbers.iter().find(|number| number["name"] == name))
        .cloned()
        .unwrap_or_else(|| panic!("no {name} in {numbers}"))
}

// One number as the select's scan holds it, derived by the Lab from what the screens show.
fn derived(name: &str, value: u64, from: &str) -> Value {
    json!({"name": name, "value": value, "origin": "derived", "from": from})
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
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_463_s29b", "Lab-CLI")
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

    fn files(&self) -> Vec<(String, Vec<u8>)> {
        files_under(&lab_dir(&self.state(), RECORD_ID))
    }

    // `record review --step <k> --no-preview`.
    fn review(&self, step: u32) -> Value {
        let asked = RecordReviewRequest {
            record_id: None,
            step: Some(step),
            no_preview: true,
            ruler: None,
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

// The first demonstration, "two forward swipes, press": steps 1 (the tap), 2 and 3 (forward to
// offsets 24 and 48), 4 (the press on `india`), 5 (the detail screen); each page marked (steps 1, 4
// and 5), then the two swipes folded into the press step 4. The list's end is on no screen.
fn two_forward() -> Bench {
    let bench = Bench::new();
    bench.press_at(START_POINT, [start(), list_at(0)]);
    for (from, to) in [(0, 24), (24, 48)] {
        bench.drag_list(FORWARD, [list_at(from), list_at(to)]);
    }
    bench.press_at(WANTED_POINT, [list_at(48), detail()]);
    bench.marked(json!({"step": 1, "add": [color("ui/start", SQUARE)]}));
    bench.marked(json!({"step": 4, "add": [color("ui/list", SQUARE)]}));
    bench.marked(json!({"step": 5, "add": [color("ui/detail", SQUARE)]}));
    bench.marked(json!({"step_action": {"kind": "fold_scan", "step": 4, "first": 2}}));
    bench
}

// The second demonstration, "three forward swipes to the end, one back, press": steps 1 (the tap),
// 2 to 4 (forward to offsets 24, 48 and 72, the list's end), 5 (back to 48), 6 (the press on
// `india`), 7 (the detail screen); each page marked (steps 1, 6 and 7), then the four swipes folded
// into the press step 6.
fn to_the_end() -> Bench {
    let bench = Bench::new();
    bench.press_at(START_POINT, [start(), list_at(0)]);
    for (swipe, (from, to)) in
        [FORWARD, FORWARD, FORWARD, BACK]
            .into_iter()
            .zip([(0, 24), (24, 48), (48, 72), (72, 48)])
    {
        bench.drag_list(swipe, [list_at(from), list_at(to)]);
    }
    bench.press_at(WANTED_POINT, [list_at(48), detail()]);
    bench.marked(json!({"step": 1, "add": [color("ui/start", SQUARE)]}));
    bench.marked(json!({"step": 6, "add": [color("ui/list", SQUARE)]}));
    bench.marked(json!({"step": 7, "add": [color("ui/detail", SQUARE)]}));
    bench.marked(json!({"step_action": {"kind": "fold_scan", "step": 6, "first": 2}}));
    bench
}

// The pressed name's decision: `india`, row 8, the scan's position 8.
fn pressed_india() -> Value {
    json!({"result": "selected", "name": "india", "position": 8})
}

// The work order's test 1: "two forward swipes, press" marked with `stop: "wanted"` and no end:
// the select stands, its click the pressed row's box; the kernel's offline entry over the three
// forward screens (each fed twice) with the default values stops early on the screen at offset 48
// where `india` first shows (coverage `early_stopped` over the ten rows read, 0 to 9), decides
// `india`, each forward swipe a step of 24 px, and runs no absent name; record ready lists nothing;
// record stop generates the scan without `end`, with `early_stop: "upper_bound"`, the select
// without `on_empty` and the task without its mapping, the kernel admits the package (its
// self-checks), and lab.selects carries the same decision.
#[test]
fn s29b_two_forward_swipes_and_the_press_without_an_end_stop_early_and_are_admitted() {
    let bench = two_forward();
    let marked = bench.marked(json!({"step": 4, "select": straight(false)}));
    let click = serde_json::to_value(&marked.click).expect("click JSON");
    assert_eq!(click["source"], "select", "{click}");
    assert_eq!(click["rect"], json!(PRESSED_ROW), "{click}");
    let select = serde_json::to_value(&marked.select).expect("select JSON");
    assert_eq!(select["self_test"]["status"], "passed", "{select}");
    assert_eq!(
        select["scan"]["outcome"],
        json!({
            "coverage": "early_stopped",
            "positions": 10,
            "pressed": pressed_india(),
            "steps": [{"step": 2, "content_step_px": 24}, {"step": 3, "content_step_px": 24}]
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
            (json!(3), json!("forward"), json!(48))
        ],
        "{select}"
    );
    assert_eq!(
        screens
            .iter()
            .map(|screen| screen["regions"].clone())
            .collect::<Vec<_>>(),
        [
            json!(["list", "row", "anchor", "name", "start"]),
            json!(["list"]),
            json!(["list"])
        ],
        "{select}"
    );

    let ready = bench.ready();
    assert_eq!(ready["missing"], json!([]), "{ready}");
    assert_eq!(ready["status"], "ready", "{ready}");

    let lab = bench.stopped();
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let task = task_of(&lab);
    let operation = select_operation(&task);
    let scan = &operation["select"]["scan"];
    assert_eq!(scan.get("end"), None, "{scan}");
    assert_eq!(scan["early_stop"], "upper_bound", "{scan}");
    assert_eq!(
        scan["start"],
        json!({"check": "select/step_02/start", "rewind": true})
    );
    assert_eq!(
        scan["bounds"],
        json!({"max_viewports": 8, "max_reposition_viewports": 6, "max_ms": 42_000})
    );
    assert_eq!(operation["select"].get("on_empty"), None, "{operation}");
    assert_eq!(
        task["scheduling_outcome"]["mappings"]
            .as_array()
            .expect("the mappings")
            .iter()
            .map(|mapping| mapping["outcome_key"].clone())
            .collect::<Vec<_>>(),
        [json!("neutral_task_done")],
        "{task}"
    );
    let checked = &lab["selects"][0];
    assert_eq!(checked["op"], "step_02_select", "{checked}");
    assert_eq!(checked["on_empty_key"], Value::Null, "{checked}");
    assert_eq!(checked["scan"]["coverage"], "early_stopped", "{checked}");
    assert_eq!(checked["scan"]["pressed"], pressed_india(), "{checked}");
    assert_eq!(checked["scan"].get("absent"), None, "{checked}");
}

// The work order's test 2: "three forward swipes to the end, one back, press" with the end mark
// given: the scan is generated with `end` and `early_stop`, the select with R1's `on_empty`; the
// offline run for `india` stops early on the screen at offset 48 (the end check, made first on each
// screen, does not pass there), and the run for `lima`, which no screen shows, reads the list to
// its end at offset 72 and decides empty over complete coverage, at mark time and in record stop's
// self-check.
#[test]
fn s29b_with_an_end_given_the_scan_has_both_and_both_offline_runs_are_made() {
    let bench = to_the_end();
    let marked = bench.marked(json!({"step": 6, "select": straight(true)}));
    let select = serde_json::to_value(&marked.select).expect("select JSON");
    assert_eq!(select["self_test"]["status"], "passed", "{select}");
    let outcome = &select["scan"]["outcome"];
    assert_eq!(outcome["coverage"], "early_stopped", "{select}");
    assert_eq!(outcome["pressed"], pressed_india(), "{select}");
    let absent = json!({"name": "lima", "coverage": "complete", "decision": {"result": "empty"}});
    assert_eq!(outcome["absent"], absent, "{select}");
    assert_eq!(
        select["scan"]["screens"][3]["regions"],
        json!(["list", "end"]),
        "{select}"
    );

    let lab = bench.stopped();
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let task = task_of(&lab);
    let operation = select_operation(&task);
    let scan = &operation["select"]["scan"];
    assert_eq!(
        scan["end"],
        json!({"check": "select/step_02/end"}),
        "{scan}"
    );
    assert_eq!(scan["early_stop"], "upper_bound", "{scan}");
    assert_eq!(
        scan["bounds"],
        json!({"max_viewports": 8, "max_reposition_viewports": 8, "max_ms": 42_000})
    );
    assert_eq!(
        operation["select"]["on_empty"],
        json!({"outcome_key": NOT_FOUND_KEY}),
        "{operation}"
    );
    let checked = &lab["selects"][0];
    assert_eq!(checked["on_empty_key"], NOT_FOUND_KEY, "{checked}");
    assert_eq!(checked["scan"]["coverage"], "early_stopped", "{checked}");
    assert_eq!(checked["scan"]["pressed"], pressed_india(), "{checked}");
    assert_eq!(checked["scan"]["absent"], absent, "{checked}");
}

// The work order's test 3: an ending "not in the list" (`on_empty_key`) beside a scan straight to
// the wanted item without an end is refused at the mark, `record_select_invalid` with
// `on_empty_without_end`, nothing written; and (the work order's point on the forward window) a
// smallest content step given above twice the position tolerance plus one, 6 above 2 x 2 + 1, is
// refused by the self-test, `scan_min_step_too_large`, nothing written.
#[test]
fn s29b_an_ending_beside_a_scan_without_an_end_is_refused_at_the_mark() {
    let bench = two_forward();
    let mut ending = straight(false);
    ending["on_empty_key"] = json!("neutral_task_absent");
    refusal(
        bench.refused(json!({"step": 4, "select": ending})),
        LabCode::RecordSelectInvalid.as_str(),
        "select_problem",
        "on_empty_without_end",
    );
    let mut wide = straight(false);
    wide["scan"]["numbers"] = json!({"advance_min_content_step_px": 6});
    let detail = rejection(
        bench.refused(json!({"step": 4, "select": wide})),
        "scan_min_step_too_large",
    );
    assert_eq!(detail["min_content_step_px"], 6, "{detail}");
    assert_eq!(detail["position_tolerance_px"], 2, "{detail}");
    assert_eq!(detail["most"], 5, "{detail}");
}

// The work order's test 4: the wanted value's default is `lima`, a name on no screen of the
// demonstration, the demonstration still pressing `india` (as a default set after the mark leaves
// it): the mark stands (it presses `india`), but a run with the default values could only find
// `lima` beyond the recorded screens: record ready lists the step's self-check `selects`
// (`record_step_self_mismatch`), its one item, and record stop and its dry run refuse with the
// reason `scan_default_not_shown`, nothing written.
#[test]
fn s29b_a_wanted_default_no_screen_shows_fails_the_self_check() {
    let bench = two_forward();
    let mut select = straight(false);
    select["wanted"]["default"] = json!("lima");
    select["pressed"] = json!("india");
    let marked = bench.marked(json!({"step": 4, "select": select}));
    let scan = serde_json::to_value(&marked.select).expect("select JSON")["scan"].clone();
    assert_eq!(scan["outcome"]["pressed"], pressed_india(), "{scan}");

    let ready = bench.ready();
    assert_eq!(ready["status"], "draft", "{ready}");
    assert_eq!(ready["missing_total"], 1, "{ready}");
    let item = &ready["missing"][0];
    assert_eq!(item["what"], "selects", "{item}");
    assert_eq!(item["code"], "record_step_self_mismatch", "{item}");
    assert_eq!(item["step"], 4, "{item}");
    assert_eq!(item["artifact_step"], 2, "{item}");

    let before = bench.files();
    for dry_run in [false, true] {
        let error = bench.stop(dry_run).expect_err("a default no screen shows");
        assert_eq!(
            error.code, "record_step_self_mismatch",
            "{}: {:?}",
            error.message, error.details
        );
        let details = error.details.expect("details");
        assert_eq!(details["reason"], "scan_default_not_shown", "{details}");
        assert_eq!(details["default"], "lima", "{details}");
        assert_eq!(bench.files(), before, "dry run {dry_run}: a file changed");
    }
}

// The work order's test 5: record review's sheet of the press step says the kind (`scan.stop`),
// whether an end is declared and what a run does when the wanted item is in no row: without the end
// mark it fails with the kernel's `selection_scan_early_stop_unreached`; with it, it ends normally
// with the select's `on_empty` key. The sheet lists every number: the window 1 to 72 and the
// tolerance 2 (so 1 <= 2 x 2 + 1), as the first kind takes them; and, the demonstration stopping at
// the wanted row, the numbers derived from what its three forward screens show (the coordinator's
// ruling after S2-9a's review): the trailing gap the list's 80 px (the kernel checks it only where
// an end check passes), the most anchors (80 / 8 + 2) x 1 = 12 (a screen showed six, 6 + 2 = 8),
// the viewports 2V + 2 = 8, the return viewports 2V = 6 and the scan's time
// (8 + 6 + 1) x (300 + 1700) + 12 x 1000. With the end mark the demonstration reaches the list's
// end and the viewports are the first kind's 2V = 8 over its four forward screens.
#[test]
fn s29b_record_review_says_the_kind_the_end_and_what_a_run_does_without_the_item() {
    let bench = two_forward();
    bench.marked(json!({"step": 4, "select": straight(false)}));
    let review = bench.review(4);
    let sheet = &review["steps"][0]["scan"];
    assert_eq!(sheet["scan"]["stop"], "wanted", "{sheet}");
    assert_eq!(sheet["scan"].get("end"), None, "{sheet}");
    assert_eq!(
        sheet["when_absent"],
        json!({"kind": "wanted", "end_declared": false, "run": "fails",
               "fails_with": "selection_scan_early_stop_unreached"}),
        "{sheet}"
    );
    assert_eq!(sheet["outcome"]["coverage"], "early_stopped", "{sheet}");
    let numbers = &sheet["numbers"];
    for (name, value) in [
        ("advance_min_content_step_px", 1),
        ("advance_max_content_step_px", 72),
        ("position_tolerance_px", 2),
    ] {
        assert_eq!(number(numbers, name)["value"], value, "{sheet}");
    }
    for (name, value, from) in [
        ("spacing_max_trailing_gap_px", 80, "list_and_row"),
        ("max_instances", 12, "list_and_row"),
        ("max_viewports", 8, "fold_swipes"),
        ("max_reposition_viewports", 6, "fold_swipes"),
        ("max_ms", 42_000, "scan_moves"),
    ] {
        assert_eq!(number(numbers, name), derived(name, value, from), "{sheet}");
    }

    let ended = to_the_end();
    ended.marked(json!({"step": 6, "select": straight(true)}));
    let review = ended.review(6);
    let sheet = &review["steps"][0]["scan"];
    assert_eq!(
        sheet["when_absent"],
        json!({"kind": "wanted", "end_declared": true, "run": "ends_normally",
               "outcome_key": NOT_FOUND_KEY}),
        "{sheet}"
    );
    assert_eq!(
        number(&sheet["numbers"], "max_viewports")["value"],
        8,
        "{sheet}"
    );
}
