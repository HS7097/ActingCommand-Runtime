// SPDX-License-Identifier: AGPL-3.0-only

// #463 SEL-2L-a (contracts/lab-recording.md, "Selecting in a list", "Reading names by detecting
// blocks"; register §1.479; brief rd-1118/brief-463-sel2l-a-fixed-blocks.md; design
// rd-1118/design-463-sel2l.md, its slices S-count and S-fixed): a select's `read` may declare
// `"blocks": "detect"` and `"line_merge"`, the words of a layout's shared read. record stop then
// generates the select's fixed_slots layout with `shared_ocr` over the bounding rectangle of the
// slots' name boxes, its members the read's, and the mark's self-test reads through the same
// generated probe as the run will. Without the two members everything is what it was. The refusal
// of a mark counts marks on both sides of its message.
//
// Frames are synthetic: 96x64 pixels of flat gray, two slots (8, 8, 32, 32) and (48, 8, 32, 32),
// their name boxes (10, 10, 28, 16) and (50, 10, 28, 16), whose bounding rectangle is
// (10, 10, 68, 16). A name is drawn as horizontal runs of a marker gray, 12 pixels long from
// x = 12 + 40k: a name of two lines NORTH on row 13 above GATE on row 20, a name of one line on
// row 17, across the middle of the box. The scripted provider, asked to detect, reads each run as
// one box (its rectangle, its marker's text and confidence); a region it is asked to read whole is
// one line, so runs on two rows of it read as one block at confidence 0.3, as a one-line
// recogniser reads two lines; a region without a run reads nothing. It keeps every request's
// region and block mode. The colour mark state/page on a flat area recognises the list and guards
// the select. No device is used; the tests read no clock and never sleep. Every expected value
// comes from the brief's and the design's lines, the contract's "What record stop generates" and
// the values above, never from the code.
//
// What the provider is asked during a mark's self-test (read in the kernel, finishing round): each
// dry run of the select (`dry_run_select`) recognises the probe's pages by their colour marks, which
// call no provider, and projects the layout; a slot target the layout's shared read serves takes its
// text from that frame's one read (`shared_text`, `read_shared_text` in the recognition pack's
// `candidate_layout.rs`), and only a target it does not serve would read its name box alone. The
// generator serves every slot, so every request of a select with `blocks` is the shared read
// (SHARED, detect with the declared merge): one per dry run, a dry run per frame for the default
// and one more for the pressed name.

use crate::codes::{LabCode, SelectProblem};
use crate::recording::crosscheck::admit;
use crate::recording::marks::MarkRejection;
use crate::recording::select::{NAME_UNREAD, SELECT_REJECTION_ID};
use crate::recording::steps::rejected;
use crate::recording::store::{old_record_path, recording_path};
use crate::{
    ClickEffect, CommitClickRequest, LAB_RECORD_MARK_SCHEMA, LabOcr, MarkOutcome, MarkRequest,
    PlanClickRequest, RecordRect, RecordStartOptions, RecordStopOptions, RecordingLock,
    record_commit_click, record_mark, record_plan_click, record_start, record_stop,
};
use actingcommand_contract::{InputAction, LabError};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_execution_kernel::{OfflineDecision, simulate_contained_task_at};
use actingcommand_pack_containment::{
    ContainmentLimits, ContentContainer, expand_content_container,
};
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderBlocks, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, PackRect, VisionProvider, VisionProviderError, VisionProviderErrorCode,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

const INSTANCE: &str = "neutral.instance";
const RECORD_ID: &str = "record_sel2l";
const TASK_ID: &str = "neutral_task";
const WIDTH: u32 = 96;
const HEIGHT: u32 = 64;
const BACKGROUND: u8 = 60;
// The gray of the next screen's page area, which the list's colour mark does not take.
const ELSEWHERE: u8 = 170;
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PAGE: &str = "state/page";
const NEXT_SEED: u64 = 0x0463_0479;

// The gray of a marker run, the text the provider reads from it and its confidence.
const NORTH: u8 = 211;
const GATE: u8 = 212;
const BRAVO: u8 = 213;
const CARGO: u8 = 214;
const DIM_BRAVO: u8 = 215;
const CARGO_A: u8 = 216;
const MARKERS: [(u8, &str, f32); 6] = [
    (NORTH, "NORTH", 0.9),
    (GATE, "GATE", 0.9),
    (BRAVO, "BRAVO", 0.9),
    (CARGO, "CARGO", 0.9),
    (DIM_BRAVO, "BRAVO", 0.5),
    (CARGO_A, "CARGOA", 0.9),
];

// The rows of a name: the two lines of a two-line name and the middle row of a one-line name; a
// run's length; the start and length of a run that reaches from slot 1 into slot 0's name box.
const LINE_ONE: u32 = 13;
const LINE_TWO: u32 = 20;
const MIDDLE: u32 = 17;
const RUN: u32 = 12;
const REACH_X: u32 = 34;
const REACH_RUN: u32 = 38;

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

// The slots' rectangles, their name boxes and the bounding rectangle of the name boxes.
const SLOTS: [RecordRect; 2] = [rect(8, 8, 32, 32), rect(48, 8, 32, 32)];
const NAMES: [RecordRect; 2] = [rect(10, 10, 28, 16), rect(50, 10, 28, 16)];
const SHARED: RecordRect = rect(10, 10, 68, 16);

// Detects each marker run of the requested region as one box; reads a region asked whole as one
// line. Keeps every request's region and block mode.
#[derive(Debug, Default)]
struct BlocksOcr {
    requests: Mutex<Vec<(PackRect, OcrProviderBlocks)>>,
}

impl BlocksOcr {
    fn requests(&self) -> Vec<(PackRect, OcrProviderBlocks)> {
        self.requests.lock().expect("the requests").clone()
    }
}

// The marker at (x, y) of the request's frame, if its gray is one.
fn marker_at(request: &OcrProviderRequest<'_>, x: i32, y: i32) -> Option<(u8, &'static str, f32)> {
    let index = (y as usize * request.frame.width as usize + x as usize) * 3;
    let pixel = request.frame.rgb8_pixels.get(index).copied()?;
    MARKERS
        .iter()
        .copied()
        .find(|(value, _, _)| *value == pixel)
}

// Each run of one marker gray on one row of the requested region, as one block.
fn runs(request: &OcrProviderRequest<'_>) -> Vec<OcrProviderTextBlock> {
    let region = request.region;
    let (right, bottom) = (region.x + region.width, region.y + region.height);
    let mut blocks = Vec::new();
    for y in region.y..bottom {
        let mut x = region.x;
        while x < right {
            let Some((value, text, confidence)) = marker_at(request, x, y) else {
                x += 1;
                continue;
            };
            let start = x;
            while x < right && marker_at(request, x, y).is_some_and(|(next, _, _)| next == value) {
                x += 1;
            }
            blocks.push(OcrProviderTextBlock {
                text: text.to_owned(),
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

impl VisionProvider for BlocksOcr {
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
    // Nothing.
    Empty,
    // A name of two lines, NORTH above GATE.
    TwoLines,
    // A name of one line, a run of the marker across the middle of the box.
    Centred(u8),
    // A name of one line across the middle, its run starting at x 34, inside slot 0's name box,
    // and its centre in slot 1's.
    Reaching(u8),
}

fn draw(values: &mut [u8], marker: u8, (x, y): (u32, u32), length: u32) {
    for column in x..x + length {
        values[(y * WIDTH + column) as usize] = marker;
    }
}

// The list showing in slot `k` its title.
fn list(titles: [Title; 2]) -> Vec<u8> {
    let mut values = vec![BACKGROUND; (WIDTH * HEIGHT) as usize];
    for (slot, title) in (0_u32..).zip(titles) {
        let x = 12 + 40 * slot;
        match title {
            Title::Empty => {}
            Title::TwoLines => {
                draw(&mut values, NORTH, (x, LINE_ONE), RUN);
                draw(&mut values, GATE, (x, LINE_TWO), RUN);
            }
            Title::Centred(marker) => draw(&mut values, marker, (x, MIDDLE), RUN),
            Title::Reaching(marker) => draw(&mut values, marker, (REACH_X, MIDDLE), REACH_RUN),
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

// The page mark of the list: a color mark on a flat area no marker reaches.
fn page_mark() -> Value {
    json!({"id": PAGE, "family": "color", "region": rect(2, 50, 4, 4)})
}

// The read of the selects: today's members, then `extra` in their order.
fn read_with(extra: &[(&str, Value)]) -> Value {
    let mut read = json!({"languages": ["en"], "timeout_ms": 1000, "minimum_confidence_milli": 800,
                          "model_ref": "neutral-ocr", "model_sha256": MODEL_SHA256});
    for (member, value) in extra {
        read[*member] = value.clone();
    }
    read
}

// The read by detecting blocks, their lines merged by `line_merge`.
fn detecting(line_merge: Value) -> Value {
    read_with(&[("blocks", json!("detect")), ("line_merge", line_merge)])
}

fn names() -> Value {
    json!([{"id": "item_a", "aliases": ["NORTHGATE"]}, {"id": "item_b", "aliases": ["BRAVO"]},
           {"id": "item_c", "aliases": ["CARGOA"]}, {"id": "item_d", "aliases": ["CARGOB"]}])
}

// The select over the two slots, its members in the order a declaration is written.
fn select(read: Value, pressed: &str, default: &str) -> Value {
    json!({
        "slots": [{"rect": SLOTS[0], "name": NAMES[0]}, {"rect": SLOTS[1], "name": NAMES[1]}],
        "read": read,
        "names": names(),
        "match": {"max_distance": 1, "minimum_margin": 1},
        "wanted": {"name": "list_item", "default": default},
        "pressed": pressed
    })
}

// One frame's decisions: the pressed name's and the default's, each `selected` with its slot.
fn decisions(frame_id: &str, pressed: u32, default: u32) -> Value {
    json!({"frame_id": frame_id, "pressed": {"result": "selected", "slot": pressed},
           "default": {"result": "selected", "slot": default}})
}

// Today's golden of the select's layout ("What record stop generates").
fn golden_layout() -> Value {
    json!({
        "id": "layout/step_01",
        "page_id": "step_01",
        "kind": "fixed_slots",
        "features": [{
            "name": "name",
            "value": "identity",
            "identity": {
                "entries": names(),
                "recognition": {"kind": "ocr_aliases", "max_distance": 1, "minimum_margin": 1,
                                "minimum_confidence_milli": 800}
            }
        }],
        "slots": [
            {"rect": SLOTS[0], "click": SLOTS[0], "targets": {"name": "select/step_01/slot_00"}},
            {"rect": SLOTS[1], "click": SLOTS[1], "targets": {"name": "select/step_01/slot_01"}}
        ]
    })
}

// Today's golden of the slots' name targets ("What record stop generates").
fn golden_targets() -> Value {
    Value::Array(
        (0..2)
            .map(|slot| {
                json!({
                    "id": format!("select/step_01/slot_{slot:02}"),
                    "region": {"mode": "rect", "rect": NAMES[slot]},
                    "languages": ["en"],
                    "timeout_ms": 1000,
                    "match_mode": "contains",
                    "expected": ["NORTHGATE", "BRAVO", "CARGOA", "CARGOB"],
                    "case_sensitive": false,
                    "minimum_confidence": 0.8,
                    "model_ref": "neutral-ocr",
                    "model_sha256": MODEL_SHA256
                })
            })
            .collect(),
    )
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).expect("JSON text")
}

fn request(mut value: Value) -> MarkRequest {
    value["schema_version"] = json!(LAB_RECORD_MARK_SCHEMA);
    serde_json::from_value(value).expect("a record mark request")
}

fn inside(point: (i32, i32), area: RecordRect) -> bool {
    point.0 >= area.x
        && point.1 >= area.y
        && point.0 < area.x + area.width
        && point.1 < area.y + area.height
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
    provider: Arc<BlocksOcr>,
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
        let lock = RecordingLock::acquire(&state, INSTANCE, "gate_sel2l", "Lab-CLI")
            .expect("recording lock");
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
        let provider = Arc::new(BlocksOcr::default());
        Self {
            lock,
            ocr: LabOcr::with_provider(provider.clone()),
            provider,
            temp,
        }
    }

    // Writes `bytes` as the frame file `name` and returns its path.
    fn frame(&self, name: &str, bytes: &[u8]) -> String {
        let path = self.temp.path().join(format!("{name}.png"));
        fs::write(&path, bytes).expect("frame file");
        path.display().to_string()
    }

    fn mark(&self, value: Value) -> Result<MarkOutcome, LabError> {
        record_mark(&self.lock, &request(value), false, &self.ocr)
    }

    // Step 1 on `primary` with its samples: the page mark and the select.
    fn step_one(&self, primary: &[u8], samples: &[&[u8]], select: Value) -> Value {
        json!({
            "frame": self.frame("primary", primary),
            "samples": samples
                .iter()
                .enumerate()
                .map(|(index, sample)| self.frame(&format!("sample_{index}"), sample))
                .collect::<Vec<_>>(),
            "add": [page_mark()],
            "select": select
        })
    }

    // The final step: the next screen with its own template mark.
    fn mark_next_step(&self) {
        let frame = self.frame("next", &next_screen());
        self.mark(json!({
            "frame": frame,
            "add": [{"id": "ui/next", "family": "template", "region": rect(40, 46, 12, 12),
                     "search": rect(40, 46, 12, 12)}]
        }))
        .unwrap_or_else(|error| panic!("record mark of the next step: {}", error.code));
    }

    // The click of the open step planned and committed as performed, which closes the step;
    // returns the pressed point.
    fn press_open_step(&self) -> (i32, i32) {
        let plan = record_plan_click(&self.lock, &PlanClickRequest::default())
            .unwrap_or_else(|error| panic!("plan click: {}", error.code));
        record_commit_click(&self.lock, &plan, &performed())
            .unwrap_or_else(|error| panic!("commit click: {}", error.code));
        (plan.point.x, plan.point.y)
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

    // The files of the package `record stop` wrote.
    fn package(&self, lab: &Value) -> BTreeMap<String, Vec<u8>> {
        let bytes = fs::read(lab["path"].as_str().expect("package path")).expect("package");
        expand_content_container(&bytes, ContentContainer::Zip, ContainmentLimits::default())
            .expect("package container")
    }

    fn recording_bytes(&self) -> Vec<u8> {
        fs::read(recording_path(&self.temp.path().join("state"), RECORD_ID))
            .expect("recording file")
    }

    fn recording(&self) -> Value {
        serde_json::from_slice(&self.recording_bytes()).expect("recording")
    }
}

fn task_text(entries: &BTreeMap<String, Vec<u8>>) -> String {
    let bytes = entries
        .get(&format!("resources/operations/{TASK_ID}/task.json"))
        .expect("task.json");
    String::from_utf8(bytes.clone()).expect("task text")
}

fn task(entries: &BTreeMap<String, Vec<u8>>) -> Value {
    serde_json::from_str(&task_text(entries)).expect("task JSON")
}

// The `select_problem` of a `record_select_invalid` refusal, its outcome carrying the same word.
fn select_problem(error: LabError) -> String {
    assert_eq!(
        error.code,
        LabCode::RecordSelectInvalid.as_str(),
        "{}",
        error.message
    );
    let details = error.details.expect("details");
    assert_eq!(
        details["outcome"]["values"]["select_problem"], details["select_problem"],
        "{details}"
    );
    details["select_problem"]
        .as_str()
        .expect("a word")
        .to_owned()
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
        })
        .cloned()
        .unwrap_or_else(|| panic!("no select {reason} rejection in {details}"))
}

// Test 1 of the brief: a select without the two members reads each name box alone, as today, and
// the mark's answer, the recording and the generated layout and targets are today's golden, byte
// for byte.
#[test]
fn sel2l_without_the_members_the_package_and_the_answer_are_todays() {
    let recording = Recording::new();
    let declared = select(read_with(&[]), "item_b", "item_c");
    let marked = recording
        .mark(recording.step_one(
            &list([Title::Centred(BRAVO), Title::Centred(CARGO_A)]),
            &[],
            declared.clone(),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));

    let requests = recording.provider.requests();
    assert!(
        !requests.is_empty()
            && requests.iter().all(|(region, blocks)| {
                *blocks == OcrProviderBlocks::Implicit
                    && NAMES.iter().any(|name| *region == pack(*name))
            }),
        "{requests:?}"
    );
    let answer = serde_json::to_value(&marked.select).expect("select JSON");
    let golden = json!({
        "declaration": declared,
        "self_test": {"status": "passed", "frames": [decisions("f0001", 0, 1)]}
    });
    assert_eq!(
        serde_json::to_string(&json!({
            "declaration": answer["declaration"],
            "self_test": answer["self_test"]
        }))
        .expect("answer text"),
        serde_json::to_string(&golden).expect("golden text")
    );
    let click = serde_json::to_value(&marked.click).expect("click JSON");
    assert_eq!(click["source"], "select", "{click}");
    assert_eq!(click["rect"], json!(SLOTS[0]), "{click}");
    assert_eq!(
        serde_json::to_string(&recording.recording()["steps"][0]["select"]["declaration"])
            .expect("stored text"),
        serde_json::to_string(&declared).expect("declared text")
    );
    let text = String::from_utf8(recording.recording_bytes()).expect("recording text");
    assert!(!text.contains("line_merge"), "{text}");

    recording.press_open_step();
    recording.mark_next_step();
    let lab = recording.stop();

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let entries = recording.package(&lab);
    let raw = task_text(&entries);
    assert!(
        !raw.contains("shared_ocr") && !raw.contains("line_merge"),
        "{raw}"
    );
    let task = task(&entries);
    assert_eq!(
        pretty(&task["candidate_layouts"]),
        pretty(&json!([golden_layout()]))
    );
    assert_eq!(pretty(&task["ocr_targets"]), pretty(&golden_targets()));
}

// Test 2 of the brief, a two-line name, a one-line name across the middle and both in one row of
// slots: read whole, the two-line name reads poorly (select_name_unread, low_confidence, as on
// production); with `blocks` the self-test reads both on both frames through one detection over the
// bounding rectangle of the name boxes with the declared merge, record stop generates `shared_ocr`
// with the read's members beside today's layout and targets, and the package taps the default's
// slot on each screen.
#[test]
fn sel2l_blocks_read_two_line_and_centred_names_in_one_row_as_the_run_reads_them() {
    let primary = list([Title::TwoLines, Title::Centred(BRAVO)]);
    let sample = list([Title::Centred(BRAVO), Title::TwoLines]);
    let today = Recording::new();
    let error = today
        .mark(today.step_one(
            &primary,
            &[&sample],
            select(read_with(&[]), "item_a", "item_b"),
        ))
        .expect_err("a two-line name read as one line");
    let refused = select_rejection(error, "select_name_unread");
    assert_eq!(refused["slot"], 0, "{refused}");
    assert_eq!(refused["cause"], "low_confidence", "{refused}");

    let recording = Recording::new();
    let declared = select(detecting(json!({"gap_factor": 0})), "item_a", "item_b");
    let marked = recording
        .mark(recording.step_one(&primary, &[&sample], declared.clone()))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));

    let marked_select = serde_json::to_value(&marked.select).expect("select JSON");
    assert_eq!(
        marked_select["self_test"]["status"], "passed",
        "{marked_select}"
    );
    assert_eq!(
        marked_select["self_test"]["frames"],
        json!([decisions("f0001", 0, 1), decisions("f0002", 1, 0)]),
        "{marked_select}"
    );
    let click = serde_json::to_value(&marked.click).expect("click JSON");
    assert_eq!(click["rect"], json!(SLOTS[0]), "{click}");
    // Two frames, each read by the default's dry run and the pressed name's: four calls, each the
    // shared read; no slot's name box is read alone.
    let requests = recording.provider.requests();
    let shared = (
        pack(SHARED),
        OcrProviderBlocks::Detect {
            line_merge_gap_factor: Some(0),
        },
    );
    assert!(
        requests.len() == 4 && requests.iter().all(|request| *request == shared),
        "{requests:?}"
    );
    assert_eq!(
        recording.recording()["steps"][0]["select"]["declaration"],
        declared
    );
    let point = recording.press_open_step();
    assert!(inside(point, SLOTS[0]), "{point:?}");
    recording.mark_next_step();

    let lab = recording.stop();

    assert_eq!(lab["lab_status"], "generated", "{lab}");
    let entries = recording.package(&lab);
    let task = task(&entries);
    let mut layout = golden_layout();
    layout["shared_ocr"] = json!({
        "region": {"mode": "rect", "rect": SHARED},
        "languages": ["en"],
        "timeout_ms": 1000,
        "model_ref": "neutral-ocr",
        "model_sha256": MODEL_SHA256,
        "blocks": "detect",
        "line_merge": {"gap_factor": 0}
    });
    assert_eq!(pretty(&task["candidate_layouts"]), pretty(&json!([layout])));
    assert_eq!(pretty(&task["ocr_targets"]), pretty(&golden_targets()));
    let prepared =
        admit(&entries, &recording.ocr).unwrap_or_else(|error| panic!("admission: {}", error.code));
    for (screen, slot) in [(primary, SLOTS[1]), (sample, SLOTS[0])] {
        let frame = Frame::from_png(screen, CaptureBackendName::AdbScreencap).expect("frame");
        match simulate_contained_task_at(&prepared, vec![frame], 1)
            .expect("simulation")
            .decision
        {
            OfflineDecision::WouldClick {
                operation_label,
                action: InputAction::Tap { x, y },
                ..
            } => {
                assert_eq!(operation_label, "step_01_select");
                assert!(inside((x, y), slot), "{x},{y}");
            }
            other => panic!("not a tap: {other:?}"),
        }
    }
}

// Test 2 of the brief, a neighbour's text close by that belongs to the next slot: slot 1's name
// reaches into slot 0's name box with its centre in its own. Read whole, slot 0's box holds three
// rows and reads poorly; detected, each block goes to the slot its centre lies in, and the merge
// `none` is the request's.
#[test]
fn sel2l_a_neighbours_text_belongs_to_the_slot_its_centre_lies_in() {
    let primary = list([Title::TwoLines, Title::Reaching(BRAVO)]);
    let today = Recording::new();
    let error = today
        .mark(today.step_one(&primary, &[], select(read_with(&[]), "item_a", "item_b")))
        .expect_err("slot 0's box read whole holds a part of slot 1's name");
    let refused = select_rejection(error, "select_name_unread");
    assert_eq!(refused["slot"], 0, "{refused}");
    assert_eq!(refused["cause"], "low_confidence", "{refused}");

    let recording = Recording::new();
    let marked = recording
        .mark(recording.step_one(
            &primary,
            &[],
            select(detecting(json!("none")), "item_a", "item_b"),
        ))
        .unwrap_or_else(|error| panic!("record mark: {} {:?}", error.code, error.details));

    let marked_select = serde_json::to_value(&marked.select).expect("select JSON");
    assert_eq!(
        marked_select["self_test"]["frames"],
        json!([decisions("f0001", 0, 1)]),
        "{marked_select}"
    );
    // One frame, read by the default's dry run and the pressed name's: two calls, each the shared
    // read with no merge.
    let requests = recording.provider.requests();
    let shared = (
        pack(SHARED),
        OcrProviderBlocks::Detect {
            line_merge_gap_factor: None,
        },
    );
    assert!(
        requests.len() == 2 && requests.iter().all(|request| *request == shared),
        "{requests:?}"
    );
}

// Test 2 of the brief, a slot with no name, two names within the margin and a low confidence: with
// `blocks`, slot 0's two-line name reads and slot 1 reads no known name, select_name_unread with
// slot 1 alone and its cause (no block reads no confidence: low_confidence, as the design's table
// says of today), the default's decision blocked; nothing is written.
#[test]
fn sel2l_a_missing_name_two_near_names_and_a_low_confidence_stay_unread() {
    let recording = Recording::new();
    let declared = select(detecting(json!({"gap_factor": 0})), "item_a", "item_b");
    for (second, cause) in [
        (Title::Empty, "low_confidence"),
        (Title::Centred(CARGO), "ambiguous"),
        (Title::Centred(DIM_BRAVO), "low_confidence"),
    ] {
        let error = recording
            .mark(recording.step_one(&list([Title::TwoLines, second]), &[], declared.clone()))
            .expect_err("slot 1 reads no known name");
        let refused = select_rejection(error, "select_name_unread");
        assert_eq!(refused["slot"], 1, "{refused}");
        assert_eq!(refused["cause"], cause, "{refused}");
        assert_eq!(
            refused["unread"],
            json!([{"slot": 1, "cause": cause}]),
            "{refused}"
        );
        assert_eq!(
            refused["default"],
            json!({"result": "blocked"}),
            "{refused}"
        );
        assert_eq!(recording.recording()["steps"], json!([]), "nothing written");
    }
}

// Test 3 of the brief: `blocks` without `line_merge` and the reverse are the incomplete
// declaration's validation_failed, the message naming the member that is missing; `blocks` on a
// select of rows is record_select_invalid with blocks_with_rows; nothing is written.
#[test]
fn sel2l_blocks_and_line_merge_come_together_and_not_on_rows() {
    let recording = Recording::new();
    let primary = list([Title::TwoLines, Title::Centred(BRAVO)]);
    for (read, missing) in [
        (
            read_with(&[("blocks", json!("detect"))]),
            "blocks without line_merge",
        ),
        (
            read_with(&[("line_merge", json!({"gap_factor": 0}))]),
            "line_merge without blocks",
        ),
    ] {
        let error = recording
            .mark(recording.step_one(&primary, &[], select(read, "item_a", "item_b")))
            .expect_err("an incomplete declaration");
        assert_eq!(error.code, "validation_failed", "{}", error.message);
        assert!(error.message.contains(missing), "{}", error.message);
        assert_eq!(recording.recording()["steps"], json!([]), "nothing written");
    }
    let rows = json!({
        "rows": {"row": SLOTS[0], "anchor": rect(30, 30, 8, 8), "name": NAMES[0]},
        "scan": {"list": rect(0, 0, 96, 48), "start": rect(88, 0, 8, 8), "stop": "end"},
        "read": detecting(json!({"gap_factor": 0})),
        "names": names(),
        "match": {"max_distance": 1, "minimum_margin": 1},
        "wanted": {"name": "list_item", "default": "item_b"},
        "pressed": "item_a"
    });
    let error = recording
        .mark(recording.step_one(&primary, &[], rows))
        .expect_err("a select of rows with blocks");
    assert_eq!(
        select_problem(error),
        SelectProblem::BlocksWithRows.as_str()
    );
    assert_eq!(recording.recording()["steps"], json!([]), "nothing written");
}

// Test 4 of the brief: one select mark refused on several frames counts once: "1 of 2" for the
// page's colour mark and the select, the three refusals all in details; a scrolling select's
// refusals, one per screen and row (23 screens of 16 rows, the observed 368), are "1 of 4", and
// another mark's refusal beside them makes "2 of 4".
#[test]
fn sel2l_the_refusal_counts_marks_not_frames_screens_or_rows() {
    let recording = Recording::new();
    let error = recording
        .mark(recording.step_one(
            &list([Title::TwoLines, Title::Empty]),
            &[
                &list([Title::Empty, Title::Centred(BRAVO)]),
                &list([Title::TwoLines, Title::Centred(CARGO)]),
            ],
            select(detecting(json!({"gap_factor": 0})), "item_a", "item_b"),
        ))
        .expect_err("a slot of every frame reads no known name");
    assert_eq!(error.code, "record_mark_rejected", "{}", error.message);
    assert_eq!(error.message, "1 of 2 marks failed; nothing recorded");
    let marks = error
        .details
        .as_ref()
        .and_then(|details| details["marks"].as_array())
        .cloned()
        .expect("the refusals");
    assert_eq!(marks.len(), 3, "{marks:?}");
    assert!(
        marks
            .iter()
            .all(|mark| mark["id"] == "select" && mark["reason"] == "select_name_unread"),
        "{marks:?}"
    );

    let mut rejections = (1..=23)
        .flat_map(|screen| {
            (0..16).map(move |row| {
                MarkRejection::new(
                    SELECT_REJECTION_ID,
                    NAME_UNREAD,
                    json!({"frame_id": format!("f{screen:04}"), "row": row,
                           "cause": "low_confidence"}),
                )
            })
        })
        .collect::<Vec<_>>();
    let refused = rejected(&rejections, 4);
    assert_eq!(refused.message, "1 of 4 marks failed; nothing recorded");
    assert_eq!(
        refused
            .details
            .as_ref()
            .and_then(|details| details["marks"].as_array())
            .map(Vec::len),
        Some(368)
    );
    rejections.push(MarkRejection::new(
        "ui/title",
        "self_test_failed",
        json!({}),
    ));
    assert_eq!(
        rejected(&rejections, 4).message,
        "2 of 4 marks failed; nothing recorded"
    );
}
