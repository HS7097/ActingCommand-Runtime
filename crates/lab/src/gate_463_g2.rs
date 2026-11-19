// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 SEL-GRID G2 (contracts/lab-recording.md, "Reading names by detecting blocks" and
// the refusal `select_name_unread`; register §1.491; the coordinator's work order
// rd-1119/brief-463-g2-name-lines.md, its point 8 and its tests of the Lab): a select's `read` may
// declare `lines` beside `blocks` and `line_merge`, in the words of a layout's `shared_ocr.lines`;
// record stop then writes them into the shared read it generates, and the mark's self-test reads
// through the same probe as the run will. Without the member nothing is generated of it. When a
// slot's name is refused, the refusal lists what the slot's read saw: each block's rectangle,
// height and text, and a cut block's strips; a name that passed the confidence and matched no name
// within the distance has a cause of its own, `beyond_distance`.
//
// Frames are synthetic: 96x64 pixels of flat gray, two slots (4, 4, 44, 40) and (50, 4, 44, 40),
// their name boxes (6, 6, 40, 30) and (52, 6, 40, 30), whose bounding rectangle is (6, 6, 86, 30).
// A line of a name is drawn as a band of vertical stripes of one marker gray, every second column
// of x 10..40 (56..86 in slot 1): a name of two lines on rows 8..18 and 20..30, 2 px apart, a name
// of one line on rows 14..24. The scripted provider's detector finds each band of one marker as a
// block and, as the detector merges two lines this close, two bands of one slot as one block; its
// recognizer reads a region as a recognizer of one line reads it: one band whole at its marker's
// confidence, parts of two run together at 0.3. Asked with lines, it plans each block by the vision
// boundary's own planner (`ocr_name_line_plan`) on the frame it is given and reads a cut block strip
// by strip. The colour mark state/page on a flat area recognises the list and guards the select.
// No device is used; the tests read no clock and never sleep. Every expected value comes from the
// brief's lines and the values above.

use crate::recording::store::{old_record_path, recording_path};
use crate::{
    ClickEffect, CommitClickRequest, LAB_RECORD_MARK_SCHEMA, LabOcr, MarkOutcome, MarkRequest,
    PlanClickRequest, RecordRect, RecordStartOptions, RecordStopOptions, RecordingLock,
    record_commit_click, record_mark, record_plan_click, record_start, record_stop,
};
use actingcommand_contract::LabError;
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderLines, OcrProviderLinesResult,
    OcrProviderRequest, OcrProviderResult, OcrProviderTextBlock, PackRect, VisionProvider,
    VisionProviderError, VisionProviderErrorCode,
};
use actingcommand_vision_ffi::{
    OcrNameLinePlan, OcrNameLines, VisionFrameView, VisionPixelFormat, VisionRect,
    ocr_name_line_plan,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::sync::Arc;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_g2";
const TASK_ID: &str = "neutral_task";
const WIDTH: u32 = 96;
const HEIGHT: u32 = 64;
const BACKGROUND: u8 = 60;
const ELSEWHERE: u8 = 170;
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PAGE: &str = "state/page";
const NEXT_SEED: u64 = 0x0463_0491;

// The gray of a marker, the text the provider reads from it and its confidence.
const NORTH: u8 = 211;
const GATE: u8 = 212;
const BRAVO: u8 = 213;
const ZULU: u8 = 214;
const YANKEE: u8 = 215;
const DIM_BRAVO: u8 = 216;
const MARKERS: [(u8, &str, f32); 6] = [
    (NORTH, "NORTH", 0.9),
    (GATE, "GATE", 0.9),
    (BRAVO, "BRAVO", 0.9),
    (ZULU, "ZULU", 0.9),
    (YANKEE, "YANKEE", 0.9),
    (DIM_BRAVO, "BRAVO", 0.5),
];

const fn rect(x: i32, y: i32, width: i32, height: i32) -> RecordRect {
    RecordRect {
        x,
        y,
        width,
        height,
    }
}

const SLOTS: [RecordRect; 2] = [rect(4, 4, 44, 40), rect(50, 4, 44, 40)];
const NAMES: [RecordRect; 2] = [rect(6, 6, 40, 30), rect(52, 6, 40, 30)];
/// Slot 0's name of two lines found as one block: rows 8..30, its stripes x 10..38.
const MERGED: PackRect = PackRect {
    x: 10,
    y: 8,
    width: 29,
    height: 22,
};

/// One band of one marker inside a request's region: its bounding rectangle, its text and its
/// confidence.
#[derive(Debug, Clone, Copy)]
struct Band {
    rect: PackRect,
    text: &'static str,
    confidence: f32,
}

/// The bands of the request's region, one a marker gray found in it.
fn bands(request: &OcrProviderRequest<'_>) -> Vec<Band> {
    let region = request.region;
    let mut found = BTreeMap::<u8, (i32, i32, i32, i32)>::new();
    for y in region.y..region.y + region.height {
        for x in region.x..region.x + region.width {
            let index = (y as usize * request.frame.width as usize + x as usize) * 3;
            let Some(value) = request.frame.rgb8_pixels.get(index).copied() else {
                continue;
            };
            if MARKERS.iter().any(|(marker, _, _)| *marker == value) {
                let bounds = found.entry(value).or_insert((x, y, x, y));
                *bounds = (
                    bounds.0.min(x),
                    bounds.1.min(y),
                    bounds.2.max(x),
                    bounds.3.max(y),
                );
            }
        }
    }
    found
        .into_iter()
        .filter_map(|(value, (left, top, right, bottom))| {
            let (_, text, confidence) = MARKERS
                .iter()
                .copied()
                .find(|(marker, _, _)| *marker == value)?;
            Some(Band {
                rect: PackRect {
                    x: left,
                    y: top,
                    width: right - left + 1,
                    height: bottom - top + 1,
                },
                text,
                confidence,
            })
        })
        .collect()
}

/// The blocks the detector finds: each band, and two bands of one column at most 3 px apart as one
/// block, as the detector merges two lines this close.
fn detected(bands: &[Band]) -> Vec<PackRect> {
    let mut blocks: Vec<PackRect> = Vec::new();
    for band in bands {
        let close = blocks.iter_mut().find(|block| {
            block.x < band.rect.x + band.rect.width
                && band.rect.x < block.x + block.width
                && (band.rect.y - (block.y + block.height)).abs() <= 3
        });
        match close {
            Some(block) => {
                let bottom = (block.y + block.height).max(band.rect.y + band.rect.height);
                block.y = block.y.min(band.rect.y);
                block.height = bottom - block.y;
            }
            None => blocks.push(band.rect),
        }
    }
    blocks
}

/// What a recognizer of one line reads in `region`: the band it holds whole, or the bands it holds
/// parts of run together at 0.3; nothing without a band.
fn recognize(bands: &[Band], region: PackRect) -> Option<OcrProviderTextBlock> {
    let held = bands
        .iter()
        .filter(|band| {
            band.rect.x < region.x + region.width
                && region.x < band.rect.x + band.rect.width
                && band.rect.y < region.y + region.height
                && region.y < band.rect.y + band.rect.height
        })
        .collect::<Vec<_>>();
    let (text, confidence) = match held.as_slice() {
        [] => return None,
        [band]
            if band.rect.y >= region.y
                && band.rect.y + band.rect.height <= region.y + region.height =>
        {
            (band.text.to_owned(), band.confidence)
        }
        more => (more.iter().map(|band| band.text).collect(), 0.3),
    };
    Some(OcrProviderTextBlock {
        text,
        rect: region,
        confidence: Some(confidence),
    })
}

fn vision(rect: PackRect) -> VisionRect {
    VisionRect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

fn pack(rect: VisionRect) -> PackRect {
    PackRect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

fn text_of(blocks: &[OcrProviderTextBlock]) -> String {
    blocks
        .iter()
        .map(|block| block.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The scripted provider of the module comment.
#[derive(Debug, Default)]
struct LinesOcr;

impl VisionProvider for LinesOcr {
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
        let bands = bands(&request);
        let blocks = detected(&bands)
            .into_iter()
            .filter_map(|block| recognize(&bands, block))
            .collect::<Vec<_>>();
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: text_of(&blocks),
            confidence: Some(0.9),
            blocks,
            detection: None,
        })
    }

    fn read_text_lines(
        &self,
        request: OcrProviderRequest<'_>,
        lines: OcrProviderLines,
    ) -> Result<OcrProviderLinesResult, VisionProviderError> {
        let bands = bands(&request);
        let frame = VisionFrameView::new(
            request.frame.width,
            request.frame.height,
            VisionPixelFormat::Rgb8,
            request.frame.rgb8_pixels,
        )
        .map_err(|error| {
            VisionProviderError::new(VisionProviderErrorCode::Internal, error.message())
        })?;
        let lines = OcrNameLines {
            max: lines.max,
            line_height: lines.line_height,
        };
        let mut blocks = Vec::new();
        let mut strips = Vec::new();
        let mut below_floor = Vec::new();
        for block in detected(&bands) {
            match ocr_name_line_plan(&frame, vision(block), lines) {
                OcrNameLinePlan::BelowFloor => below_floor.push(block),
                OcrNameLinePlan::Whole => {
                    if let Some(read) = recognize(&bands, block) {
                        blocks.push(read);
                        strips.push(Vec::new());
                    }
                }
                OcrNameLinePlan::Cut(cut) => {
                    let read = cut
                        .into_iter()
                        .filter_map(|strip| recognize(&bands, pack(strip)))
                        .collect::<Vec<_>>();
                    if !read.is_empty() {
                        blocks.push(OcrProviderTextBlock {
                            text: text_of(&read),
                            rect: block,
                            confidence: read
                                .iter()
                                .filter_map(|strip| strip.confidence)
                                .reduce(f32::min),
                        });
                        strips.push(read);
                    }
                }
            }
        }
        Ok(OcrProviderLinesResult {
            result: OcrProviderResult {
                ppocr_diagnostics: Vec::new(),
                text: text_of(&blocks),
                confidence: Some(0.9),
                blocks,
                detection: None,
            },
            strips,
            below_floor,
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

// A deterministic generator of neutral gray noise below every marker value.
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

// The PNG of a gray screen, row-major values drawn with equal red, green and blue.
fn png(values: &[u8]) -> Vec<u8> {
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

// What a slot's name box shows.
#[derive(Clone, Copy)]
enum Title {
    // A name of two lines, the first marker above the second.
    TwoLines(u8, u8),
    // A name of one line across the middle of the box.
    One(u8),
}

// A band of `marker`'s stripes on rows `top..top + 10` from x `left`.
fn band(values: &mut [u8], marker: u8, left: u32, top: u32) {
    for y in top..top + 10 {
        for x in (left..left + 30).step_by(2) {
            values[(y * WIDTH + x) as usize] = marker;
        }
    }
}

// The list showing in slot `k` its title.
fn list(titles: [Title; 2]) -> Vec<u8> {
    let mut values = vec![BACKGROUND; (WIDTH * HEIGHT) as usize];
    for (slot, title) in (0_u32..).zip(titles) {
        let left = 10 + 46 * slot;
        match title {
            Title::TwoLines(first, second) => {
                band(&mut values, first, left, 8);
                band(&mut values, second, left, 20);
            }
            Title::One(marker) => band(&mut values, marker, left, 14),
        }
    }
    png(&values)
}

// The next screen: a noise patch at (40, 46), with no marker value, and its page area another
// gray.
fn next_screen() -> Vec<u8> {
    let mut noise = Noise(NEXT_SEED);
    let mut values = vec![BACKGROUND; (WIDTH * HEIGHT) as usize];
    for row in 0..12 {
        for column in 0..12 {
            values[((46 + row) * WIDTH + 40 + column) as usize] = noise.between(1, 199);
        }
    }
    for row in 50..54 {
        for column in 2..6 {
            values[(row * WIDTH + column) as usize] = ELSEWHERE;
        }
    }
    png(&values)
}

fn page_mark() -> Value {
    json!({"id": PAGE, "family": "color", "region": rect(2, 50, 4, 4)})
}

// The read of the selects, by detecting blocks merged by a factor of 0, with `lines` when given.
fn read(lines: Option<Value>) -> Value {
    let mut read = json!({"languages": ["en"], "timeout_ms": 1000, "minimum_confidence_milli": 800,
                          "model_ref": "neutral-ocr", "model_sha256": MODEL_SHA256,
                          "blocks": "detect", "line_merge": {"gap_factor": 0}});
    if let Some(lines) = lines {
        read["lines"] = lines;
    }
    read
}

fn lines_of_two() -> Value {
    json!({"max": 2, "line_height": 10})
}

fn names() -> Value {
    json!([{"id": "item_a", "aliases": ["NORTH GATE"]}, {"id": "item_b", "aliases": ["BRAVO"]},
           {"id": "item_c", "aliases": ["CARGO"]}])
}

fn select(read: Value) -> Value {
    json!({
        "slots": [{"rect": SLOTS[0], "name": NAMES[0]}, {"rect": SLOTS[1], "name": NAMES[1]}],
        "read": read,
        "names": names(),
        "match": {"max_distance": 1, "minimum_margin": 1},
        "wanted": {"name": "list_item", "default": "item_b"},
        "pressed": "item_a"
    })
}

fn request(mut value: Value) -> MarkRequest {
    value["schema_version"] = json!(LAB_RECORD_MARK_SCHEMA);
    serde_json::from_value(value).expect("a record mark request")
}

fn performed() -> CommitClickRequest {
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

// A state directory with an active session of `INSTANCE` and its Lab recording, before any step,
// with the scripted provider. The lock is dropped before the directory.
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
        let lock =
            RecordingLock::acquire(&state, INSTANCE, "gate_g2", "Lab-CLI").expect("recording lock");
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
        Self {
            lock,
            ocr: LabOcr::with_provider(Arc::new(LinesOcr)),
            temp,
        }
    }

    fn frame(&self, name: &str, bytes: &[u8]) -> String {
        let path = self.temp.path().join(format!("{name}.png"));
        fs::write(&path, bytes).expect("frame file");
        path.display().to_string()
    }

    fn mark(&self, value: Value) -> Result<MarkOutcome, LabError> {
        record_mark(&self.lock, &request(value), false, &self.ocr)
    }

    fn step_one(&self, primary: &[u8], select: Value) -> Value {
        json!({
            "frame": self.frame("primary", primary),
            "add": [page_mark()],
            "select": select
        })
    }

    fn mark_next_step(&self) {
        let frame = self.frame("next", &next_screen());
        self.mark(json!({
            "frame": frame,
            "add": [{"id": "ui/next", "family": "template", "region": rect(40, 46, 12, 12),
                     "search": rect(40, 46, 12, 12)}]
        }))
        .unwrap_or_else(|error| panic!("record mark of the next step: {}", error.code));
    }

    fn press_open_step(&self) {
        let plan = record_plan_click(&self.lock, &PlanClickRequest::default())
            .unwrap_or_else(|error| panic!("plan click: {}", error.code));
        record_commit_click(&self.lock, &plan, &performed())
            .unwrap_or_else(|error| panic!("commit click: {}", error.code));
    }

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

    fn task(&self, lab: &Value) -> Value {
        let bytes = fs::read(lab["path"].as_str().expect("package path")).expect("package");
        let entries =
            expand_content_container(&bytes, ContentContainer::Zip, ContainmentLimits::default())
                .expect("package container");
        let task = entries
            .get(&format!("resources/operations/{TASK_ID}/task.json"))
            .expect("task.json");
        serde_json::from_slice(task).expect("task JSON")
    }

    fn recording(&self) -> Value {
        serde_json::from_slice(
            &fs::read(recording_path(&self.temp.path().join("state"), RECORD_ID))
                .expect("recording file"),
        )
        .expect("recording")
    }
}

// The refused select of `record_mark_rejected` with `reason`.
fn select_rejection(error: LabError, reason: &str) -> Value {
    assert_eq!(error.code, "record_mark_rejected", "{}", error.message);
    let details = error.details.expect("details");
    details["marks"]
        .as_array()
        .and_then(|marks| {
            marks
                .iter()
                .find(|mark| mark["id"] == "select" && mark["reason"] == reason)
                .cloned()
        })
        .unwrap_or_else(|| panic!("no select {reason} rejection in {details}"))
}

// Points 8 and the Lab's tests of the brief: a select whose read declares `lines` {max 2,
// line_height 10} reads slot 0's two lines found as one block through a cut (NORTH\nGATE, item_a)
// and passes its self-test; the recording keeps the member as declared and record stop writes it
// into the generated shared read, after `line_merge`. The same select without the member generates
// a shared read without it: the self-test reads the block as one line, poorly, and refuses slot 0
// (select_name_unread, low_confidence), listing the block its read saw, its rectangle, height and
// text.
#[test]
fn g2_a_select_declares_the_lines_of_its_name_boxes_and_the_package_carries_them() {
    let primary = list([Title::TwoLines(NORTH, GATE), Title::One(BRAVO)]);

    let today = Recording::new();
    let error = today
        .mark(today.step_one(&primary, select(read(None))))
        .expect_err("two lines found as one block, read as one line");
    let refused = select_rejection(error, "select_name_unread");
    assert_eq!(refused["slot"], 0, "{refused}");
    assert_eq!(refused["cause"], "low_confidence", "{refused}");
    assert_eq!(
        refused["blocks"],
        json!([{"rect": MERGED, "height": 22, "text": "NORTHGATE"}]),
        "{refused}"
    );
    assert!(refused.get("below_floor").is_none(), "{refused}");

    let recording = Recording::new();
    let declared = select(read(Some(lines_of_two())));
    let marked = recording
        .mark(recording.step_one(&primary, declared.clone()))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));
    let marked_select = serde_json::to_value(&marked.select).expect("select JSON");
    assert_eq!(
        marked_select["self_test"]["status"], "passed",
        "{marked_select}"
    );
    assert_eq!(
        recording.recording()["steps"][0]["select"]["declaration"],
        declared
    );
    recording.press_open_step();
    recording.mark_next_step();

    let lab = recording.stop();

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let task = recording.task(&lab);
    let shared = &task["candidate_layouts"][0]["shared_ocr"];
    assert_eq!(
        shared,
        &json!({
            "region": {"mode": "rect", "rect": rect(6, 6, 86, 30)},
            "languages": ["en"],
            "timeout_ms": 1000,
            "model_ref": "neutral-ocr",
            "model_sha256": MODEL_SHA256,
            "blocks": "detect",
            "line_merge": {"gap_factor": 0},
            "lines": {"max": 2, "line_height": 10}
        })
    );
    assert_eq!(
        shared
            .as_object()
            .and_then(|members| members.keys().last())
            .map(String::as_str),
        Some("lines")
    );
}

// Point 8 of the brief, the refusal: slot 0's two lines read a name of no entry at confidence 0.9
// (ZULU over YANKEE). With the lines the block is cut and read "ZULU\nYANKEE", which passes the
// confidence and matches no name within the distance: select_name_unread with the cause
// beyond_distance, listing the cut block (its rectangle, height and text) and its two strips, each
// with its rectangle, height and text. A name read below the confidence (BRAVO at 0.5) keeps the
// cause low_confidence and lists its block read whole.
#[test]
fn g2_the_refusal_of_a_name_lists_its_blocks_and_tells_the_distance_from_the_confidence() {
    let recording = Recording::new();
    let declared = select(read(Some(lines_of_two())));

    let error = recording
        .mark(recording.step_one(
            &list([Title::TwoLines(ZULU, YANKEE), Title::One(BRAVO)]),
            declared.clone(),
        ))
        .expect_err("slot 0 reads no known name");
    let refused = select_rejection(error, "select_name_unread");
    assert_eq!(refused["slot"], 0, "{refused}");
    assert_eq!(refused["cause"], "beyond_distance", "{refused}");
    assert_eq!(
        refused["blocks"],
        json!([{
            "rect": MERGED,
            "height": 22,
            "text": "ZULU\nYANKEE",
            "strips": [
                {"rect": {"x": 10, "y": 8, "width": 29, "height": 11}, "height": 11,
                 "text": "ZULU"},
                {"rect": {"x": 10, "y": 19, "width": 29, "height": 11}, "height": 11,
                 "text": "YANKEE"}
            ]
        }]),
        "{refused}"
    );

    let error = recording
        .mark(recording.step_one(
            &list([Title::TwoLines(NORTH, GATE), Title::One(DIM_BRAVO)]),
            declared,
        ))
        .expect_err("slot 1 reads below the confidence");
    let refused = select_rejection(error, "select_name_unread");
    assert_eq!(refused["slot"], 1, "{refused}");
    assert_eq!(refused["cause"], "low_confidence", "{refused}");
    assert_eq!(
        refused["blocks"],
        json!([{
            "rect": {"x": 56, "y": 14, "width": 29, "height": 10},
            "height": 10,
            "text": "BRAVO"
        }]),
        "{refused}"
    );
    assert_eq!(recording.recording()["steps"], json!([]), "nothing written");
}

// The read's own rule: `lines` comes with `blocks` and `line_merge`, the members of the shared read
// whose name boxes it describes; alone it is validation_failed, its message naming `blocks`, and
// nothing is written.
#[test]
fn g2_lines_without_blocks_are_refused() {
    let recording = Recording::new();
    let mut alone = json!({"languages": ["en"], "timeout_ms": 1000,
                           "minimum_confidence_milli": 800, "model_ref": "neutral-ocr",
                           "model_sha256": MODEL_SHA256});
    alone["lines"] = lines_of_two();

    let error = recording
        .mark(recording.step_one(
            &list([Title::TwoLines(NORTH, GATE), Title::One(BRAVO)]),
            select(alone),
        ))
        .expect_err("lines without blocks");

    assert_eq!(error.code, "validation_failed", "{}", error.message);
    assert!(
        error.message.contains("lines without blocks"),
        "{}",
        error.message
    );
    assert_eq!(recording.recording()["steps"], json!([]), "nothing written");
}
