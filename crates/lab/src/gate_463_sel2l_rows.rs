// SPDX-License-Identifier: AGPL-3.0-only

// #463 SEL-2L-rows (contracts/lab-recording.md, "Reading names by detecting blocks" and "A
// scrolling choice: reading the list to its end"; register §1.484; brief
// rd-1119/brief-463-sel2l-rows-generator.md; design rd-1118/design-463-sel2l.md, its slice S-rows):
// a scrolling select's `read` may declare `"blocks": "detect"` and `line_merge`, as a select of
// fixed slots' may. record stop then generates its repeated_anchor layout with `shared_ocr` over
// the list, the layout's readable band, after its features, its members the read's, and the mark's
// self-test, record ready and record stop read every screen through the same generated probe as a
// run reads it. Without the two members everything is what it was. The refusal `blocks_with_rows`
// is gone. A scan always declares its list, so no layout the Lab generates lacks a band: the case of
// a scan without a band does not exist here, and no test is written for it.
//
// Recordings are made as stage one makes them (taps and swipes through `record_press` with a fake
// Runtime whose captures take the scripted screens, the caller's clock moving only by its waits),
// as #463 S2-9a's gate makes them. Frames are synthetic 128x128 screens: a page square at (0, 0,
// 16, 16), red on the start screen, green on the list, yellow on the detail screen; on the list
// screens a list area at rows 20 to 99 that shows the content rows `offset` to `offset + 79`. The
// content is a grid of cards 56x16, two columns at x 2 and 66 and six rows at content row 4 + 20r,
// card k = 2r + c. Each card shows an anchor of noise (8x6 at its (2, 0)) and its name inside its
// name box (40x12 at its (14, 2)): on two lines, its first word on the card's row 5 and its second
// on row 10, where r + c is even, else on one line, both words on row 8, so that every row of the
// grid holds a two-line name and a one-line name. A line is a run of 20 pixels of one marker gray
// from the card's x 16. A start mark of noise at content rows 0 to 3 and an end mark of other noise
// at content rows 128 to 133, both at x 60 to 75. The demonstration: a tap on the start screen opens
// the list at offset 0 (step 1), two swipes forward of 30 px (steps 2 and 3: offsets 30 and 60, the
// list's end), one back (step 4: offset 30), the press on `hotel` (card 7, a two-line name) at
// (90, 62) (step 5), the detail screen (step 6). At offset 30 the grid's sixth row (cards 10 and
// 11) stands at frame rows 94 to 109: its anchors lie in the list and are found, the band's edge
// cuts the cards, and the first line of card 11's name (frame row 99) lies inside the read.
//
// The scripted provider, asked to detect, reads each run of one marker gray on one pixel row of the
// requested region as one block (its rectangle, its word, confidence 0.9; 0.3 for the smeared grays
// of a screen captured while the list still moved); a region it is asked to read whole is one line,
// so runs on two rows of it read as one block at confidence 0.3, as a one-line recogniser reads two
// lines. It keeps every request's region and block mode. No Runtime, no device; the tests read no
// clock and never sleep. The decisions, names and refusals come from the brief, the contract and
// the geometry above. The golden layout and name target are the contract's "What record stop
// generates" for a scrolling select written out member by member in the order the generator writes
// them (`scan::layout_json`, `scan::name_target`), so that they pin today's bytes.

use crate::codes::SelectProblem;
use crate::recording::crosscheck::admit;
use crate::recording::store::{lab_dir, old_record_path, recording_path};
use crate::{
    AfterCapture, AttachFrameRequest, CapturedFrame, ClickEffect, ClickPlan, CommitClickRequest,
    LAB_RECORD_MARK_SCHEMA, LabOcr, MarkOutcome, MarkRequest, PlanClickRequest, PlannedSwipe,
    PointPress, RecordFramePorts, RecordPoint, RecordPressRequest, RecordRect, RecordStartOptions,
    RecordStopOptions, RecordingLock, record_mark, record_press, record_ready, record_start,
    record_stop,
};
use actingcommand_contract::{InputFrameReference, LabError, LabResult, package_edge_holds};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderBlocks, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, PackRect, VisionProvider, VisionProviderError, VisionProviderErrorCode,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_0463_sel2l_rows";
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

const fn pack(rect: RecordRect) -> PackRect {
    PackRect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

/// Each screen's page square.
const SQUARE: RecordRect = rect(0, 0, 16, 16);
/// The list's area: the frame's rows 20 to 99, the layout's readable band and the shared read's
/// region.
const LIST_TOP: u32 = 20;
const LIST_HEIGHT: u32 = 80;
const LIST: RecordRect = rect(0, 20, 128, 80);
/// The grid: six rows of cards 20 px apart, the first at content row 4, two columns.
const CARD_ROWS: u32 = 6;
const PITCH: u32 = 20;
const FIRST_ROW: u32 = 4;
const COLUMNS: [u32; 2] = [2, 66];
const CARD_WIDTH: u32 = 56;
const CARD_HEIGHT: u32 = 16;
/// The names of the cards, in card order; card k shows the words `TOPS[k]` and `BOTTOMS[k]`, its
/// alias their concatenation.
const IDS: [&str; 12] = [
    "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliet",
    "kilo", "lima",
];
const TOPS: [&str; 12] = [
    "ALPHA", "BRAVO", "CHARLIE", "DELTA", "ECHO", "FOXTROT", "GOLF", "HOTEL", "INDIA", "JULIET",
    "KILO", "LIMA",
];
const BOTTOMS: [&str; 12] = [
    "ONE", "TWO", "THREE", "FOUR", "FIVE", "SIX", "SEVEN", "EIGHT", "NINE", "TEN", "ELEVEN",
    "TWELVE",
];
/// The marker grays of card k: its one-line name (100 + k), the first line of its two-line name
/// (120 + k) and the second (140 + k); on a screen still moving each 60 more (smeared).
const WHOLE: u8 = 100;
const TOP: u8 = 120;
const BOTTOM: u8 = 140;
const SMEARED: u8 = 60;
/// The start and end marks: noise at x 60 to 75, content rows 0 to 3 and 128 to 133.
const MARK_X: u32 = 60;
const MARK_WIDTH: u32 = 16;
const START_ROWS: u32 = 4;
const END_TOP: u32 = 128;
const END_ROWS: u32 = 6;

/// The agent's regions on the list's first screen (offset 0): card 0 at frame row 24.
const ROW: RecordRect = rect(2, 24, 56, 16);
const ANCHOR: RecordRect = rect(4, 24, 8, 6);
const NAME: RecordRect = rect(16, 26, 40, 12);
const START_MARK: RecordRect = rect(60, 20, 16, 4);
/// The end mark on the last screen after a forward swipe (offset 60): content row 128 at 88.
const END_MARK: RecordRect = rect(60, 88, 16, 6);

/// The tap on the start screen, and the press on `hotel` (card 7: row 3, column 1, at frame rows
/// 54 to 69 on the screen at offset 30, where the press is made).
const START_POINT: (i32, i32) = (100, 110);
const ROW_POINT: (i32, i32) = (90, 62);
const PRESSED_CARD: RecordRect = rect(66, 54, 56, 16);

/// A swipe by its two points: up the list by 30 px, and back down by as much.
type Swipe = ((i32, i32), (i32, i32));
const FORWARD: Swipe = ((64, 80), (64, 50));
const BACK: Swipe = ((64, 50), (64, 80));

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

/// How the list's names are drawn: two-line and one-line names in every row of the grid, or every
/// name on one line.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Names {
    Mixed,
    OneLine,
}

// Whether card `k` shows its name on two lines in the grid of `Names::Mixed`: its row and column
// add up to an even number.
fn two_lines(k: u32) -> bool {
    (k / 2 + k % 2).is_multiple_of(2)
}

// The marker gray of the pixel at the card's row `dy` of card `k`'s name, on a line of it.
fn name_gray(k: u32, dy: u32, names: Names, smeared: bool) -> Option<u8> {
    let base = match (names == Names::Mixed && two_lines(k), dy) {
        (true, 5) => TOP,
        (true, 10) => BOTTOM,
        (false, 8) => WHOLE,
        _ => return None,
    };
    let shift = if smeared { SMEARED } else { 0 };
    Some(base + u8::try_from(k).expect("a card") + shift)
}

// The list scrolled by `offset` px, its names drawn as `names` says, smeared when `smeared`.
fn list_drawn(offset: u32, names: Names, smeared: bool) -> Vec<u8> {
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
        for r in 0..CARD_ROWS {
            let top = FIRST_ROW + PITCH * r;
            if !(top..top + CARD_HEIGHT).contains(&row) {
                continue;
            }
            for (c, left) in (0_u32..).zip(COLUMNS) {
                if !(left..left + CARD_WIDTH).contains(&x) {
                    continue;
                }
                let (dx, dy) = (x - left, row - top);
                if (2..10).contains(&dx) && dy < 6 {
                    return noise(dx - 2, dy);
                }
                if (16..36).contains(&dx)
                    && let Some(gray) = name_gray(2 * r + c, dy, names, smeared)
                {
                    return [gray; 3];
                }
            }
        }
        GRAY
    })
}

// The word a marker gray stands for and the confidence it is read at.
fn marker(gray: u8) -> Option<(String, f32)> {
    let (settled, confidence) = if gray >= WHOLE + SMEARED {
        (gray - SMEARED, 0.3)
    } else {
        (gray, 0.9)
    };
    let k = usize::from(settled % 20);
    if k >= IDS.len() {
        return None;
    }
    let word = match settled - settled % 20 {
        WHOLE => format!("{}{}", TOPS[k], BOTTOMS[k]),
        TOP => TOPS[k].to_owned(),
        BOTTOM => BOTTOMS[k].to_owned(),
        _ => return None,
    };
    Some((word, confidence))
}

// The marker gray of the pixel at (x, y) of the request's frame, if it is one: a gray pixel (its
// three values equal) of a marker's value. The anchors' and marks' noise is no gray.
fn gray_at(request: &OcrProviderRequest<'_>, x: i32, y: i32) -> Option<u8> {
    let index = (y as usize * request.frame.width as usize + x as usize) * 3;
    let pixel = request.frame.rgb8_pixels.get(index..index + 3)?;
    (pixel[0] == pixel[1] && pixel[1] == pixel[2] && marker(pixel[0]).is_some()).then_some(pixel[0])
}

// Each run of one marker gray on one pixel row of the requested region, as one block.
fn runs(request: &OcrProviderRequest<'_>) -> Vec<OcrProviderTextBlock> {
    let region = request.region;
    let (right, bottom) = (region.x + region.width, region.y + region.height);
    let mut blocks = Vec::new();
    for y in region.y..bottom {
        let mut x = region.x;
        while x < right {
            let Some(gray) = gray_at(request, x, y) else {
                x += 1;
                continue;
            };
            let start = x;
            while x < right && gray_at(request, x, y) == Some(gray) {
                x += 1;
            }
            let (text, confidence) = marker(gray).expect("a marker");
            blocks.push(OcrProviderTextBlock {
                text,
                rect: PackRect {
                    x: start,
                    y,
                    width: x - start,
                    height: 1,
                },
                confidence: Some(confidence),
            });
        }
    }
    blocks
}

// Detects each marker run of the requested region as one block; reads a region asked whole as one
// line. Keeps every request's region and block mode.
#[derive(Debug, Default)]
struct RowsOcr {
    requests: Mutex<Vec<(PackRect, OcrProviderBlocks)>>,
}

impl RowsOcr {
    fn requests(&self) -> Vec<(PackRect, OcrProviderBlocks)> {
        self.requests.lock().expect("the requests").clone()
    }
}

impl VisionProvider for RowsOcr {
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
        self.requests
            .lock()
            .expect("the requests")
            .push((request.region, request.blocks));
        let runs = runs(&request);
        let lines = runs
            .iter()
            .map(|block| block.rect.y)
            .collect::<BTreeSet<_>>()
            .len();
        let blocks = match request.blocks {
            OcrProviderBlocks::Detect { .. } => runs,
            // A region read whole is one line: two lines in it read as one poor block.
            OcrProviderBlocks::Implicit | OcrProviderBlocks::Region if lines > 1 => {
                vec![OcrProviderTextBlock {
                    text: runs.iter().map(|block| block.text.as_str()).collect(),
                    rect: request.region,
                    confidence: Some(0.3),
                }]
            }
            OcrProviderBlocks::Implicit | OcrProviderBlocks::Region => runs,
        };
        let text = blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let confidence = blocks
            .iter()
            .filter_map(|block| block.confidence)
            .reduce(f32::min);
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            confidence,
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

// The shared read's request: the detector over the list with the declared merge, factor 0.
fn shared() -> (PackRect, OcrProviderBlocks) {
    (
        pack(LIST),
        OcrProviderBlocks::Detect {
            line_merge_gap_factor: Some(0),
        },
    )
}

// The colour mark `id` over `area`.
fn color(id: &str, area: RecordRect) -> Value {
    json!({"id": id, "family": "color", "region": area})
}

fn request(mut value: Value) -> MarkRequest {
    value["schema_version"] = json!(LAB_RECORD_MARK_SCHEMA);
    serde_json::from_value(value).expect("a record mark request")
}

// The alias of card `k`: its two words as one.
fn alias(k: usize) -> String {
    format!("{}{}", TOPS[k], BOTTOMS[k])
}

fn names_json() -> Vec<Value> {
    (0..IDS.len())
        .map(|k| json!({"id": IDS[k], "aliases": [alias(k)]}))
        .collect()
}

// The ids of cards `from` to `to - 1`, in card order.
fn ids(from: usize, to: usize) -> Value {
    json!(IDS[from..to])
}

// The read of the select: today's members, then `extra` in their order.
fn read_with(extra: &[(&str, Value)]) -> Value {
    let mut read = json!({"languages": ["en"], "timeout_ms": 1000, "minimum_confidence_milli": 800,
                          "model_ref": "neutral-ocr", "model_sha256": MODEL_SHA256});
    for (member, value) in extra {
        read[*member] = value.clone();
    }
    read
}

// The read by detecting blocks, their lines merged with the factor 0.
fn detecting() -> Value {
    read_with(&[
        ("blocks", json!("detect")),
        ("line_merge", json!({"gap_factor": 0})),
    ])
}

// The scrolling select of the first kind over the grid with `read`, the wanted value's default
// `hotel`, the card the demonstration pressed.
fn scrolling(read: Value) -> Value {
    json!({
        "rows": {"row": ROW, "anchor": ANCHOR, "name": NAME},
        "scan": {"list": LIST, "start": START_MARK, "end": END_MARK, "stop": "end"},
        "read": read,
        "names": names_json(),
        "match": {"max_distance": 1, "minimum_margin": 1},
        "wanted": {"name": "list_item", "default": "hotel"}
    })
}

// The mark request of the scrolling select `select` on the press step 5.
fn on_press(select: Value) -> Value {
    json!({"step": 5, "select": select})
}

// Today's golden of a scrolling select's layout on this grid ("What record stop generates", "A
// scrolling choice: reading the list to its end"), its `max_instances` the select's number: every
// rectangle relative to the anchor's top-left corner (4, 24) on the first screen, the list as the
// readable band.
fn golden_layout(max_instances: &Value) -> Value {
    json!({
        "id": "layout/step_02",
        "page_id": "step_02",
        "kind": "repeated_anchor",
        "anchor": "select/step_02/anchor",
        "max_instances": max_instances,
        "order": "top_to_bottom",
        "instance_rect": {"x": -2, "y": 0, "width": 56, "height": 16},
        "click": {"x": -2, "y": 0, "width": 56, "height": 16},
        "readable_band": LIST,
        "features": [{
            "name": "name",
            "value": "identity",
            "target": "select/step_02/name",
            "offset": {"x": 12, "y": 2},
            "identity": {
                "entries": names_json(),
                "recognition": {"kind": "ocr_aliases", "max_distance": 1, "minimum_margin": 1,
                                "minimum_confidence_milli": 800}
            }
        }]
    })
}

// Today's golden of the row's name target, at the name region of the first screen.
fn golden_name_target() -> Value {
    json!({
        "id": "select/step_02/name",
        "region": {"mode": "rect", "rect": NAME},
        "languages": ["en"],
        "timeout_ms": 1000,
        "match_mode": "contains",
        "expected": (0..IDS.len()).map(alias).collect::<Vec<_>>(),
        "case_sensitive": false,
        "minimum_confidence": 0.8,
        "model_ref": "neutral-ocr",
        "model_sha256": MODEL_SHA256
    })
}

// The shared read the layout gains: the list, the layout's readable band, and the read's members.
fn golden_shared_read() -> Value {
    json!({
        "region": {"mode": "rect", "rect": LIST},
        "languages": ["en"],
        "timeout_ms": 1000,
        "model_ref": "neutral-ocr",
        "model_sha256": MODEL_SHA256,
        "blocks": "detect",
        "line_merge": {"gap_factor": 0}
    })
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).expect("JSON text")
}

// The value of the scan's number `name` the select holds.
fn number(select: &Value, name: &str) -> Value {
    select["scan"]["numbers"]
        .as_array()
        .and_then(|numbers| numbers.iter().find(|number| number["name"] == name))
        .map(|number| number["value"].clone())
        .unwrap_or_else(|| panic!("no number {name} in {select}"))
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

fn task_text(entries: &BTreeMap<String, Vec<u8>>) -> String {
    let bytes = entries
        .get(&format!("resources/operations/{TASK_ID}/task.json"))
        .expect("task.json");
    String::from_utf8(bytes.clone()).expect("task text")
}

fn task_of(entries: &BTreeMap<String, Vec<u8>>) -> Value {
    serde_json::from_str(&task_text(entries)).expect("task JSON")
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

/// A state directory with an active session of `INSTANCE` and its Lab recording, its game, server
/// and locale known, with the scripted provider. The lock is dropped before the directory.
struct Bench {
    lock: RecordingLock,
    provider: Arc<RowsOcr>,
    ocr: LabOcr,
    temp: TempDir,
}

impl Bench {
    fn new() -> Self {
        let temp = TempDir::new().expect("temporary directory");
        let state = temp.path().join("state");
        fs::create_dir_all(&state).expect("state directory");
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_463_sel2l_rows", "Lab-CLI")
            .expect("recording lock");
        let provider = Arc::new(RowsOcr::default());
        let bench = Self {
            lock,
            ocr: LabOcr::with_provider(provider.clone()),
            provider,
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

    fn recording_text(&self) -> String {
        String::from_utf8(fs::read(recording_path(&self.state(), RECORD_ID)).expect("step table"))
            .expect("step table text")
    }

    fn recording(&self) -> Value {
        serde_json::from_str(&self.recording_text()).expect("step table JSON")
    }

    fn files(&self) -> Vec<(String, Vec<u8>)> {
        files_under(&lab_dir(&self.state(), RECORD_ID))
    }

    // The id of the frame the swipe of step `step` left after it.
    fn after_frame(&self, step: u32) -> String {
        let recording = self.recording();
        recording["steps"]
            .as_array()
            .and_then(|steps| steps.iter().find(|item| item["index"] == step))
            .and_then(|item| item["click"]["execution"]["after_frame"].as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| panic!("no frame after step {step} in {recording}"))
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

    // `record stop`: the `lab` output with its status.
    fn stopped(&self) -> Value {
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
}

// The demonstration on the list screens drawn as `names` says, the screen after the first forward
// swipe captured while the list still moved when `moving`: steps 1 (the tap), 2 and 3 (forward), 4
// (back), 5 (the press on `hotel`), 6 (the detail screen); the pages of steps 1, 5 and 6 marked
// and the swipes folded into the press step.
fn demonstrated(names: Names, moving: bool) -> Bench {
    let bench = Bench::new();
    let screen = |offset: u32| list_drawn(offset, names, false);
    bench.press_at(START_POINT, [start(), screen(0)]);
    bench.drag_list(FORWARD, [screen(0), list_drawn(30, names, moving)]);
    bench.drag_list(FORWARD, [screen(30), screen(60)]);
    bench.drag_list(BACK, [screen(60), screen(30)]);
    bench.press_at(ROW_POINT, [screen(30), detail()]);
    bench.marked(json!({"step": 1, "add": [color("ui/start", SQUARE)]}));
    bench.marked(json!({"step": 5, "add": [color("ui/list", SQUARE)]}));
    bench.marked(json!({"step": 6, "add": [color("ui/detail", SQUARE)]}));
    bench.marked(json!({"step_action": {"kind": "fold_scan", "step": 5, "first": 2}}));
    bench
}

// Test 1 of the brief: with `blocks`, record stop's layout is today's with `shared_ocr` after its
// features, its region the list (the readable band) and its other members the read's, the name
// target unchanged; such a layout is what the packaging's revision table counts as `package` 20
// (record stop writes no range of its own: no interfaces.json). Without the members (every name on
// one line, which a read of the name box whole reads), the recording holds no line_merge and the
// layout and the name target are today's golden, byte for byte, without shared_ocr.
#[test]
fn sel2l_rows_the_layout_gains_the_shared_read_over_its_band_and_without_blocks_is_todays() {
    let today = demonstrated(Names::OneLine, false);
    let marked = today.marked(on_press(scrolling(read_with(&[]))));
    let select = serde_json::to_value(&marked.select).expect("select JSON");
    assert_eq!(select["self_test"]["status"], "passed", "{select}");
    let recorded = today.recording_text();
    assert!(!recorded.contains("line_merge"), "{recorded}");
    let lab = today.stopped();
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let entries = package(&lab);
    let raw = task_text(&entries);
    assert!(
        !raw.contains("shared_ocr") && !raw.contains("line_merge"),
        "{raw}"
    );
    let task = task_of(&entries);
    assert_eq!(
        pretty(&task["candidate_layouts"]),
        pretty(&json!([golden_layout(&number(&select, "max_instances"))]))
    );
    assert_eq!(
        pretty(&task["ocr_targets"]),
        pretty(&json!([golden_name_target()]))
    );

    let bench = demonstrated(Names::Mixed, false);
    let marked = bench.marked(on_press(scrolling(detecting())));
    let select = serde_json::to_value(&marked.select).expect("select JSON");
    assert_eq!(select["self_test"]["status"], "passed", "{select}");
    let lab = bench.stopped();
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let entries = package(&lab);
    let task = task_of(&entries);
    let mut layout = golden_layout(&number(&select, "max_instances"));
    layout["shared_ocr"] = golden_shared_read();
    assert_eq!(pretty(&task["candidate_layouts"]), pretty(&json!([layout])));
    assert_eq!(
        pretty(&task["ocr_targets"]),
        pretty(&json!([golden_name_target()]))
    );
    assert!(
        !entries.keys().any(|path| path.ends_with("interfaces.json")),
        "{:?}",
        entries.keys().collect::<Vec<_>>()
    );
}

// Test 2 of the brief: on the grid of two-line and one-line names, every row of it holding both,
// a read of each name box whole reads a two-line name poorly (select_name_unread, low_confidence);
// with `blocks` the mark stands: on every screen each card the band holds whole reads its name, in
// the grid's order (the first screen cards 0 to 7, the screens at offset 30 cards 4 to 9, the end
// cards 6 to 11), and the cards the band's edge cuts at offset 30 (10 and 11) are not read there;
// every request of the self-test is the shared read over the list; the offline entry covers the
// list, twelve positions, each forward swipe a step of 30 px, and decides `hotel`; record ready
// lists nothing and record stop's self-checks decide `hotel` over complete coverage on the package
// it generated.
#[test]
fn sel2l_rows_every_card_the_band_holds_is_read_through_the_shared_read_and_a_cut_card_is_not() {
    let today = demonstrated(Names::Mixed, false);
    let error = today.refused(on_press(scrolling(read_with(&[]))));
    let refused = rejection(error, "select_name_unread");
    assert_eq!(refused["cause"], "low_confidence", "{refused}");

    let bench = demonstrated(Names::Mixed, false);
    let marked = bench.marked(on_press(scrolling(detecting())));
    let requests = bench.provider.requests();
    assert!(
        !requests.is_empty() && requests.iter().all(|request| *request == shared()),
        "{requests:?}"
    );
    let click = serde_json::to_value(&marked.click).expect("click JSON");
    assert_eq!(click["source"], "select", "{click}");
    assert_eq!(click["rect"], json!(PRESSED_CARD), "{click}");
    let select = serde_json::to_value(&marked.select).expect("select JSON");
    assert_eq!(select["self_test"]["status"], "passed", "{select}");
    let screens = select["scan"]["screens"].as_array().expect("the screens");
    assert_eq!(
        screens
            .iter()
            .map(|screen| (
                screen["step"].clone(),
                screen["role"].clone(),
                screen["content_offset_px"].clone(),
                screen["names"].clone()
            ))
            .collect::<Vec<_>>(),
        [
            (json!(2), json!("first"), json!(0), ids(0, 8)),
            (json!(2), json!("forward"), json!(30), ids(4, 10)),
            (json!(3), json!("forward"), json!(60), ids(6, 12)),
            (json!(4), json!("back"), Value::Null, ids(4, 10))
        ],
        "{select}"
    );
    let outcome = &select["scan"]["outcome"];
    assert_eq!(outcome["coverage"], "complete", "{select}");
    assert_eq!(outcome["positions"], 12, "{select}");
    assert_eq!(outcome["pressed"]["result"], "selected", "{select}");
    assert_eq!(outcome["pressed"]["name"], "hotel", "{select}");
    assert_eq!(
        outcome["steps"],
        json!([{"step": 2, "content_step_px": 30}, {"step": 3, "content_step_px": 30}]),
        "{select}"
    );

    let ready = bench.ready();
    assert_eq!(ready["missing"], json!([]), "{ready}");
    assert_eq!(ready["status"], "ready", "{ready}");
    let lab = bench.stopped();
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let checked = &lab["selects"][0]["scan"];
    assert_eq!(checked["coverage"], "complete", "{lab}");
    assert_eq!(checked["pressed"]["result"], "selected", "{lab}");
    assert_eq!(checked["pressed"]["name"], "hotel", "{lab}");
}

// Test 3 of the brief: the screen after the first forward swipe was captured while the list still
// moved, its names smeared (read at confidence 0.3): the mark is refused with select_name_unread on
// that screen alone, one for each card the band holds whole there (six; the two cut cards are not
// read), each low_confidence, and the message counts the select once beside the page's colour mark:
// "1 of 2 marks failed"; nothing is written.
#[test]
fn sel2l_rows_a_screen_still_moving_is_unread_there_and_counts_once() {
    let bench = demonstrated(Names::Mixed, true);
    let moving = bench.after_frame(2);
    let error = bench.refused(on_press(scrolling(detecting())));
    assert_eq!(error.code, "record_mark_rejected", "{}", error.message);
    assert_eq!(error.message, "1 of 2 marks failed; nothing recorded");
    let marks = error
        .details
        .as_ref()
        .and_then(|details| details["marks"].as_array())
        .cloned()
        .expect("the refusals");
    assert_eq!(marks.len(), 6, "{marks:?}");
    for mark in &marks {
        assert_eq!(mark["id"], "select", "{mark}");
        assert_eq!(mark["reason"], "select_name_unread", "{mark}");
        assert_eq!(mark["frame_id"], moving.as_str(), "{mark}");
        assert_eq!(mark["cause"], "low_confidence", "{mark}");
    }
    let rows = marks
        .iter()
        .map(|mark| mark["row"].to_string())
        .collect::<BTreeSet<_>>();
    assert_eq!(rows.len(), 6, "{marks:?}");
}

// Test 5 of the brief, and the end-to-end test SEL-2L-b's review asked for: the package record stop
// writes holds the source task.json with the shared read in its source form; the kernel's own
// admission (`load_content_entries`: the declaration gate, the derivation and the recognition
// pack's admission) admits it in memory, and record stop's self-checks, which ran the kernel's
// offline entry on that package, read through its shared read and decide `hotel`, a two-line name
// no read of its name box whole reads. The range half proves less than the package's revision: see
// its comment below.
#[test]
fn sel2l_rows_the_generated_package_is_admitted_at_package_20_and_refused_below() {
    let bench = demonstrated(Names::Mixed, false);
    bench.marked(on_press(scrolling(detecting())));
    let before = bench.provider.requests().len();
    let lab = bench.stopped();
    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let during = bench.provider.requests().split_off(before);
    assert!(during.contains(&shared()), "{during:?}");
    let checked = &lab["selects"][0]["scan"];
    assert_eq!(checked["coverage"], "complete", "{lab}");
    assert_eq!(checked["pressed"]["name"], "hotel", "{lab}");

    let entries = package(&lab);
    let task = task_of(&entries);
    let layout = &task["candidate_layouts"][0];
    assert_eq!(layout["kind"], "repeated_anchor", "{layout}");
    assert_eq!(
        pretty(&layout["shared_ocr"]),
        pretty(&golden_shared_read()),
        "{layout}"
    );
    admit(&entries, &bench.ocr).unwrap_or_else(|error| {
        panic!(
            "admission: {}: {} {:?}",
            error.code, error.message, error.details
        )
    });

    // What this half proves: the build's own declared `package` range is [3, 20], it holds a
    // bundle of [20, 20], and a range that ends at 19 does not (`package_edge_holds`, the rule of
    // the installer's `place` and of ready's I2). What it does not prove: that this generated
    // package needs 20. The pair [20, 20] is written by hand, after the contract's row 20
    // (contracts/component-interfaces.md). The function that computes a bundle's revision from
    // its content, `content_revision` (apps/actinglab/src/resource_verify/revision.rs, which
    // counts a `repeated_anchor` layout with `shared_ocr` as 20, `anchored_shared_read`), lives in
    // the actinglab binary, which has no library target and itself depends on this crate, so this
    // crate cannot call it; its row 20 is pinned in apps/actinglab/src/tests/gate_464_p2.rs, on a
    // layout written there, not on this package.
    let declared: Value = serde_json::from_str(include_str!(
        "../../../distribution/windows/component-interfaces.json"
    ))
    .expect("the build's declaration is JSON");
    let own = match declared["speaks"]["package"].as_array().map(Vec::as_slice) {
        Some([min, max]) => [
            min.as_u64().expect("a minimum"),
            max.as_u64().expect("a maximum"),
        ],
        other => panic!("the build's package range: {other:?}"),
    };
    // Workflow #463 SEL-GRID G2 (register §1.491) raises the build's range to [3, 21], the lines
    // of a shared read's name boxes, Workflow #428 S168-LK (register §1.500) to [3, 22], a
    // linear step's list reading, Workflow #455 G-SLx (register §1.501) to [3, 23], a pack's
    // variables beside an irreversible capability, Workflow #477 CV-1 (register §1.507) to [3, 24],
    // a bundle that carries a version 2 package declaration, Workflow #477 LR-1 (register
    // §1.513) to [3, 25], a linear press step's count, Workflow #455 TK-1 (register §1.532) to
    // [3, 26], a variable only a rule gives, Workflow #503 LIST-A (register §1.533) to [3, 27], a
    // list reading of the wanted keys only, and Workflow #455 RR1 (register §1.535) to [3, 28], a
    // `goals.json` of version 3 and a root's `goal-tables.json`; [20, 20] stays within it.
    assert_eq!(own, [3, 28]);
    assert!(package_edge_holds(own, [20, 20]));
    assert!(!package_edge_holds([3, 19], [20, 20]));
}

// Test 6 of the brief: the vocabulary select_problem of the catalog is the Lab's enum, word for
// word, and `blocks_with_rows` is in neither, nor in the Lab's fragment or its index.
#[test]
fn sel2l_rows_blocks_with_rows_is_gone_from_the_vocabulary() {
    let catalog: Value = serde_json::from_str(include_str!(
        "../../../contracts/outcome-codes/catalog.json"
    ))
    .expect("the catalog is JSON");
    let tokens = catalog["vocabularies"]["select_problem"]["tokens"]
        .as_object()
        .expect("the words of select_problem")
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let words = SelectProblem::ALL
        .iter()
        .map(|problem| problem.as_str().to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(tokens, words);
    assert!(!tokens.contains("blocks_with_rows"), "{tokens:?}");
    for (file, text) in [
        (
            "lab.json",
            include_str!("../../../contracts/outcome-codes/lab.json"),
        ),
        (
            "index/lab.md",
            include_str!("../../../contracts/outcome-codes/index/lab.md"),
        ),
    ] {
        assert!(!text.contains("blocks_with_rows"), "{file}");
    }
}
