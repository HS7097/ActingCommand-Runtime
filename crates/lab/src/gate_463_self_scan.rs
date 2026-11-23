// SPDX-License-Identifier: AGPL-3.0-only

// #463 SEL-F (contracts/lab-recording.md, "A select the package fixes"; register §1.523), the work
// order's test 5: a scrolling select whose wanted item the package fixes (`wanted.fixed`), over a
// scan read to the list's end (#463 S2-9a, no early stop) and over a scan straight to the item
// (#463 S2-9b, `early_stop: "upper_bound"`, no end mark). Recordings are made as stage one makes
// them (taps and swipes through `record_press` with a fake Runtime, its captures taking the
// scripted screens and the caller's clock moving only by its waits), on S2-9a's synthetic list:
// 128x128 screens, a page square of one colour at (0, 0, 16, 16) and, on the list screens, a list
// area at rows 20 to 99 that shows the content rows `offset` to `offset + 79`; eleven content rows
// 12 px apart (row k at content row 4 + 12k), each an anchor of noise (8x6 at x 4) and a name marker
// pixel at (20, top + 2) whose grey value the stand-in name reader reads as the row's name; a start
// mark of noise at content rows 0 to 3 and an end mark of other noise at content rows 136 to 141,
// both at x 60 to 75. Two demonstrations, as `gate_463_s29a` and `gate_463_s29b` make them: "three
// forward to the end, two back, press `delta`" (row 3, on the screen at offset 24) and "two forward,
// press `india`" (row 8, on the screen at offset 48). No Runtime, no device, no OCR model but the
// stand-in reader; the tests read no clock and never sleep.
// - `self_t5_a_fixed_select_over_a_scan_read_to_its_end_declares_no_variable`
// - `self_t5_a_fixed_select_over_a_scan_that_stops_early_declares_no_variable`
// Addendum 1, the package route on a scan (the review's item 5: tests 8 and 9 take a scan fixture):
// - `self_t8_a_scrolling_select_converted_from_its_files_is_what_a_fixed_recording_generates`
// - `self_t9_a_scrolling_select_converted_to_names_given_is_what_a_recording_fixing_them_generates`

use crate::gate_463_self::{assert_as_recorded, conversion_note, convert_package};
use crate::recording::crosscheck::admit;
use crate::recording::store::old_record_path;
use crate::{
    AfterCapture, AttachFrameRequest, CapturedFrame, ClickEffect, ClickPlan, CommitClickRequest,
    LAB_RECORD_MARK_SCHEMA, LabOcr, MarkOutcome, MarkRequest, PlanClickRequest, PlannedSwipe,
    PointPress, RecordFramePorts, RecordPoint, RecordPressRequest, RecordRect, RecordStartOptions,
    RecordStopOptions, RecordingLock, record_mark, record_press, record_start, record_stop,
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
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0463_self_scan";
const TASK_ID: &str = "neutral_task";
const STARTED_AT: u64 = 1_700_000_000_000;
const WIDTH: u32 = 128;
const HEIGHT: u32 = 128;
const GRAY: [u8; 3] = [60, 60, 60];
const RED: [u8; 3] = [230, 20, 20];
const GREEN: [u8; 3] = [20, 230, 20];
const YELLOW: [u8; 3] = [230, 230, 20];
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TASK: &str = "resources/operations/neutral_task/task.json";
const POLICY: &str = "resources/operations/neutral_task/policies/step_02_select.json";
const OPERATION: &str = "step_02_select";
/// The key of the run's normal end when the wanted name is in no row, as R1 makes it by default.
const NOT_FOUND: &str = "neutral_task_not_found";
/// The wanted value's name.
const WANTED: &str = "list_item";

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
/// The end mark on the screen at offset 72, the last after a forward swipe: content row 136 at
/// frame row 84.
const END_MARK: RecordRect = rect(60, 84, 16, 6);

/// The tap on the start screen; the press on `delta` (row 3, frame rows 36 to 43 at offset 24)
/// and on `india` (row 8, frame rows 72 to 79 at offset 48).
const START_POINT: (i32, i32) = (100, 110);
const DELTA_POINT: (i32, i32) = (10, 40);
const INDIA_POINT: (i32, i32) = (10, 76);

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
        let joined = blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            confidence: (!blocks.is_empty()).then_some(0.9),
            text: joined,
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

// The scrolling select over the list whose wanted item the package fixes, `item`, the row the
// demonstration pressed: of the first kind (`stop: "end"`, with its end mark) or, `straight`, of
// the second (`stop: "wanted"`, without one).
fn scrolling(item: &str, straight: bool) -> Value {
    scrolling_with(json!({"name": WANTED, "fixed": [item]}), straight)
}

// The same scrolling select with the wanted value `wanted` (Addendum 1: the variable form too).
fn scrolling_with(wanted: Value, straight: bool) -> Value {
    let scan = if straight {
        json!({"list": LIST, "start": START_MARK, "stop": "wanted"})
    } else {
        json!({"list": LIST, "start": START_MARK, "end": END_MARK, "stop": "end"})
    };
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
        "wanted": wanted
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
        let shot = self
            .shots
            .pop_front()
            .expect("a capture the test did not script");
        self.taken += 1;
        Ok(CapturedFrame {
            frame: AttachFrameRequest {
                png: shot,
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
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_463_self_scan", "Lab-CLI")
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

    // `record stop`, which must generate the package: the `lab` output with its status.
    fn stopped(&self) -> Value {
        record_stop(&self.lock, &RecordStopOptions::default(), &self.ocr)
            .map(|outcome| {
                let mut lab = serde_json::to_value(&outcome.lab).expect("lab JSON");
                lab["lab_status"] = json!(outcome.lab_status);
                lab
            })
            .unwrap_or_else(|error| {
                panic!(
                    "record stop: {}: {} {:?}",
                    error.code, error.message, error.details
                )
            })
    }
}

// The first kind's demonstration (`gate_463_s29a`): steps 1 (the tap), 2 to 4 (forward to offsets
// 24, 48 and 72, the list's end), 5 and 6 (back to 48 and 24), 7 (the press on `delta`), 8 (the
// detail screen); each page marked (steps 1, 7 and 8), the five swipes folded into the press step.
fn to_the_end() -> Bench {
    let bench = Bench::new();
    bench.press_at(START_POINT, [start(), list_at(0)]);
    for (swipe, (from, to)) in [FORWARD, FORWARD, FORWARD, BACK, BACK].into_iter().zip([
        (0, 24),
        (24, 48),
        (48, 72),
        (72, 48),
        (48, 24),
    ]) {
        bench.drag_list(swipe, [list_at(from), list_at(to)]);
    }
    bench.press_at(DELTA_POINT, [list_at(24), detail()]);
    bench.marked(json!({"step": 1, "add": [color("ui/start", SQUARE)]}));
    bench.marked(json!({"step": 7, "add": [color("ui/list", SQUARE)]}));
    bench.marked(json!({"step": 8, "add": [color("ui/detail", SQUARE)]}));
    bench.marked(json!({"step_action": {"kind": "fold_scan", "step": 7, "first": 2}}));
    bench
}

// The second kind's demonstration (`gate_463_s29b`): steps 1 (the tap), 2 and 3 (forward to
// offsets 24 and 48), 4 (the press on `india`), 5 (the detail screen); each page marked (steps 1, 4
// and 5), the two swipes folded into the press step. The list's end is on no screen.
fn two_forward() -> Bench {
    let bench = Bench::new();
    bench.press_at(START_POINT, [start(), list_at(0)]);
    for (from, to) in [(0, 24), (24, 48)] {
        bench.drag_list(FORWARD, [list_at(from), list_at(to)]);
    }
    bench.press_at(INDIA_POINT, [list_at(48), detail()]);
    bench.marked(json!({"step": 1, "add": [color("ui/start", SQUARE)]}));
    bench.marked(json!({"step": 4, "add": [color("ui/list", SQUARE)]}));
    bench.marked(json!({"step": 5, "add": [color("ui/detail", SQUARE)]}));
    bench.marked(json!({"step_action": {"kind": "fold_scan", "step": 4, "first": 2}}));
    bench
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

fn document(entries: &BTreeMap<String, Vec<u8>>, path: &str) -> Value {
    serde_json::from_slice(entries.get(path).expect("the document")).expect("document JSON")
}

// The select operation of the package's task.
fn select_operation(task: &Value) -> Value {
    task["operations"]
        .as_array()
        .and_then(|operations| {
            operations
                .iter()
                .find(|operation| operation["id"] == OPERATION)
        })
        .cloned()
        .unwrap_or_else(|| panic!("no select operation in {task}"))
}

// The policy document of the scrolling select of the package's step 2 in the fixed form, as the
// contract writes it ("A select the package fixes"): its id, its layout, its outcome keys, the
// field `name` over the twelve names, the gate `wanted.list_item` keeping `item` alone and the term
// `rank.list_item` giving it the place 1 under the weight −1000, no fact; `none_allowed` with one
// candidate, the lowest candidate id first.
fn fixed_policy(item: &str) -> Value {
    json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": OPERATION,
        "applies_to": {
            "candidate_layout_id": "layout/step_02",
            "outcome_keys": {
                "selected": "neutral_task_step_02_selected",
                "empty": NOT_FOUND,
                "insufficient": "neutral_task_step_02_insufficient",
                "ambiguous": "neutral_task_step_02_ambiguous",
                "unknown": "neutral_task_step_02_unknown"
            }
        },
        "fields": [{"name": "name", "value_type": {"type": "enum_string", "allowed": NAMES}}],
        "facts": [],
        "gates": [{
            "gate_id": "wanted.list_item",
            "predicate": {"kind": "string_in", "value": {"source": "field", "field": "name"},
                          "allowed": [item]},
            "on_unknown": {"kind": "drop_candidate"}
        }],
        "scoring": [{
            "term_id": "rank.list_item",
            "value": {"source": "field", "field": "name"},
            "transform": {"kind": "lookup",
                          "entries": [{"key": {"type": "string", "value": item},
                                       "value_milli": 1}]},
            "weight_milli": -1000,
            "on_unknown": {"kind": "drop_candidate"}
        }],
        "selection": {"mode": "none_allowed", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    })
}

// The bytes the generator writes a document with: pretty JSON and a final line feed.
fn written(value: &Value) -> String {
    let mut bytes = serde_json::to_vec_pretty(value).expect("JSON");
    bytes.push(b'\n');
    String::from_utf8(bytes).expect("UTF-8 text")
}

// What every package of both kinds holds for the fixed select: no `variables` in its control, the
// policy document in the fixed form of `item`, and `lab.selects` naming the fixed names.
fn assert_fixed(lab: &Value, entries: &BTreeMap<String, Vec<u8>>, item: &str) {
    let control = document(entries, "control.json");
    assert_eq!(control.get("variables"), None, "{control}");
    assert_eq!(
        String::from_utf8(entries.get(POLICY).expect("the policy").clone()).expect("UTF-8"),
        written(&fixed_policy(item))
    );
    assert_eq!(
        lab["selects"][0]["wanted"],
        json!({"name": WANTED, "fixed": [item]}),
        "{lab}"
    );
}

// The work order's test 5, a scan without an early stop: the first kind's demonstration, the
// select fixing `delta`: the mark stands, its self-test passed, the kernel's offline entry
// deciding `delta` (position 3) over complete coverage and `lima`, which no screen shows, empty;
// record stop generates the scan with its end and no `early_stop`, the select with its `on_empty`
// key, and lab.selects carries the same decision; the control declares no variable and the policy
// document is the fixed form.
#[test]
fn self_t5_a_fixed_select_over_a_scan_read_to_its_end_declares_no_variable() {
    let bench = to_the_end();
    let marked = bench.marked(json!({"step": 7, "select": scrolling("delta", false)}));
    let select = serde_json::to_value(&marked.select).expect("select JSON");
    assert_eq!(select["self_test"]["status"], "passed", "{select}");
    let outcome = &select["scan"]["outcome"];
    assert_eq!(outcome["coverage"], "complete", "{select}");
    assert_eq!(
        outcome["pressed"],
        json!({"result": "selected", "name": "delta", "position": 3}),
        "{select}"
    );
    assert_eq!(
        outcome["absent"],
        json!({"name": "lima", "coverage": "complete", "decision": {"result": "empty"}}),
        "{select}"
    );

    let lab = bench.stopped();

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let entries = package(&lab);
    let operation = select_operation(&document(&entries, TASK));
    let scan = &operation["select"]["scan"];
    assert_eq!(
        scan["end"],
        json!({"check": "select/step_02/end"}),
        "{scan}"
    );
    assert_eq!(scan.get("early_stop"), None, "{scan}");
    assert_eq!(
        operation["select"]["on_empty"],
        json!({"outcome_key": NOT_FOUND}),
        "{operation}"
    );
    let checked = &lab["selects"][0];
    assert_eq!(checked["scan"]["coverage"], "complete", "{checked}");
    assert_eq!(
        checked["scan"]["pressed"],
        json!({"result": "selected", "name": "delta", "position": 3}),
        "{checked}"
    );
    assert_fixed(&lab, &entries, "delta");
}

// The work order's test 5, a scan with an early stop: the second kind's demonstration, the select
// fixing `india` without an end mark: the mark stands, its self-test passed, the kernel's offline
// entry stopping early on the screen where `india` (position 8) shows; record stop generates the
// scan with `early_stop: "upper_bound"` and no end, the select without `on_empty`, and lab.selects
// carries the same decision (the self-check that the first name the package looks for shows on a
// forward screen passes); the control declares no variable and the policy document is the fixed
// form.
#[test]
fn self_t5_a_fixed_select_over_a_scan_that_stops_early_declares_no_variable() {
    let bench = two_forward();
    let marked = bench.marked(json!({"step": 4, "select": scrolling("india", true)}));
    let select = serde_json::to_value(&marked.select).expect("select JSON");
    assert_eq!(select["self_test"]["status"], "passed", "{select}");
    let outcome = &select["scan"]["outcome"];
    assert_eq!(outcome["coverage"], "early_stopped", "{select}");
    assert_eq!(
        outcome["pressed"],
        json!({"result": "selected", "name": "india", "position": 8}),
        "{select}"
    );

    let lab = bench.stopped();

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let entries = package(&lab);
    let operation = select_operation(&document(&entries, TASK));
    let scan = &operation["select"]["scan"];
    assert_eq!(scan.get("end"), None, "{scan}");
    assert_eq!(scan["early_stop"], "upper_bound", "{scan}");
    assert_eq!(operation["select"].get("on_empty"), None, "{operation}");
    let checked = &lab["selects"][0];
    assert_eq!(checked["scan"]["coverage"], "early_stopped", "{checked}");
    assert_eq!(
        checked["scan"]["pressed"],
        json!({"result": "selected", "name": "india", "position": 8}),
        "{checked}"
    );
    assert_fixed(&lab, &entries, "india");
}

// The variable form's wanted value, the default `default`.
fn chosen_by_a_person(default: &str) -> Value {
    json!({"name": WANTED, "default": default})
}

// The work order's test 8 on a scan (Addendum 1; the review's item 5): the second kind's
// demonstration with the variable form, the default `india`, its package converted from its own
// files with no names given: the select step's operation, `india` fixed; the package `record stop`
// generates for the same demonstration declared `fixed: [india]`, byte for byte but for the task's
// provenance (`gate_463_self`'s `assert_as_recorded`); its policy document the fixed form of
// `india`, no `variables`, its scan as before (`early_stop: "upper_bound"`, no end); admitted.
#[test]
fn self_t8_a_scrolling_select_converted_from_its_files_is_what_a_fixed_recording_generates() {
    let chosen = two_forward();
    chosen.marked(json!({"step": 4, "select": scrolling_with(chosen_by_a_person("india"), true)}));
    let old = package(&chosen.stopped());

    let (fix, new) = convert_package(&old, None, &chosen.ocr);

    assert_eq!(fix.operation, OPERATION);
    assert_eq!(fix.default, ["india"]);
    assert_eq!(fix.fixed, ["india"]);
    let recorded = two_forward();
    recorded.marked(json!({"step": 4, "select": scrolling("india", true)}));
    assert_as_recorded(
        &new,
        &package(&recorded.stopped()),
        TASK,
        &conversion_note(OPERATION, &["india"], &["india"]),
    );
    assert_eq!(document(&new, "control.json").get("variables"), None);
    assert_eq!(
        String::from_utf8(new[POLICY].clone()).expect("UTF-8"),
        written(&fixed_policy("india"))
    );
    let operation = select_operation(&document(&new, TASK));
    let scan = &operation["select"]["scan"];
    assert_eq!(scan["early_stop"], "upper_bound", "{scan}");
    assert_eq!(scan.get("end"), None, "{scan}");
    let prepared =
        admit(&new, &chosen.ocr).unwrap_or_else(|error| panic!("admission: {}", error.code));
    assert!(prepared.declared_variables().is_empty());
}

// The work order's test 9 on a scan: the first kind's demonstration with the variable form, the
// default `delta`, its package converted with the names `[delta, charlie]` given: the package
// `record stop` generates for the same demonstration declared `fixed: [delta, charlie]`, byte for
// byte but for the task's provenance; its gate keeps the two names in that order; its scan keeps its
// end; admitted.
#[test]
fn self_t9_a_scrolling_select_converted_to_names_given_is_what_a_recording_fixing_them_generates() {
    let chosen = to_the_end();
    chosen.marked(json!({"step": 7, "select": scrolling_with(chosen_by_a_person("delta"), false)}));
    let old = package(&chosen.stopped());

    let (fix, new) = convert_package(&old, Some(&["delta", "charlie"][..]), &chosen.ocr);

    assert_eq!(fix.default, ["delta"]);
    assert_eq!(fix.fixed, ["delta", "charlie"]);
    let recorded = to_the_end();
    recorded.marked(json!({
        "step": 7,
        "select": scrolling_with(json!({"name": WANTED, "fixed": ["delta", "charlie"]}), false)
    }));
    assert_as_recorded(
        &new,
        &package(&recorded.stopped()),
        TASK,
        &conversion_note(OPERATION, &["delta"], &["delta", "charlie"]),
    );
    let policy = document(&new, POLICY);
    assert_eq!(
        policy["gates"][0]["predicate"]["allowed"],
        json!(["delta", "charlie"])
    );
    let operation = select_operation(&document(&new, TASK));
    assert_eq!(
        operation["select"]["scan"]["end"],
        json!({"check": "select/step_02/end"})
    );
    let prepared =
        admit(&new, &chosen.ocr).unwrap_or_else(|error| panic!("admission: {}", error.code));
    assert!(prepared.declared_variables().is_empty());
}
