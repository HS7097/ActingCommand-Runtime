// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #428 S168-LK (register §1.500; contracts/linear-steps.md, "List reading steps";
// contracts/resource-readings.md, "List readings"; the coordinator's rulings in the work order of
// board #428): a step of a `linear_steps` task may read a list. Its declaration is the page graph's;
// it reads from the frame on which its page passed, its guard judged on every viewport, and ends as
// "List readings" says: the list whole with the step finishing on its last viewport's page, on
// which the next step decides; an unread position its warning and the run going on to its end; a
// failed scan the run's end with the scan's code at the stage `read_list`, with no retry and no
// closing decision. Admission runs the reading's member checks in a step plan too and refuses a
// reading the plan cannot run; the declaration gate refuses a `key` beside `read_list` in both
// modes; a requested key set never leaves a reading out; and the offline entry answers what the
// run records on the same viewports.
//
// Every expected value comes from those rulings and the package values below, never from the code.
// The packages are synthetic `neutral` packages admitted by the kernel's whole admission
// (`PreparedContainedTask::load_with_vision_provider`, the declaration gate included), or by the
// step plan's own admission (`validate_linear`) for a refusal. A frame is 64 x 60. Pixel (0, 0)
// names the page (home, list, detail, done); on the list pixel (1, 0) is green at the list's start,
// pixel (2, 0) blue at its end (the end check, and the guard of the step after the reading) and
// pixel (3, 0) green (the reading's guard). The list holds up to seven cards in one column, card k
// at content y 10 + 12 k: a noise marker (the anchor template, 8 x 6 px) at x 4 and a 4 x 4 patch
// of its own colour to its right, from which a scripted OCR provider reads its name. The path taps
// from home into the list (`open`), reads the list (`read_members`) and taps from the list to done
// (`close`, guarded by the list's end mark). The runs play scripted frames on the kernel's scripted
// clock (EXEC-I4): the device shows `screens[n]` after `n` inputs, each segment's last frame
// repeating.

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_contract::FactScalar;
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    VisionProviderError, VisionProviderErrorCode,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{Cursor, Write};
use std::time::SystemTime;
use zip::{ZipWriter, write::FileOptions};

const LINEAR_INVALID: &str = ExecutionKernelCode::ContainedTaskLinearInvalid.as_str();
const READING_INVALID: &str = ExecutionKernelCode::ContainedTaskResourceReadingInvalid.as_str();
const UNRESOLVED: &str = ExecutionKernelCode::ContainedTaskResourceReadingUnresolved.as_str();
const ALIGNMENT_REJECTED: &str = ExecutionKernelCode::SelectionScanAlignmentRejected.as_str();

const WIDTH: u32 = 64;
const HEIGHT: u32 = 60;
const LIST_KEY: &str = "inventory.list.example_kind_a";
const VALID_FOR_MS: u64 = 86_400_000;
// The detector pages of the path.
const LIST_PAGE: &str = "neutral/list";
const DONE_PAGE: &str = "neutral/done";
// The cards' names, top to bottom, and the business IDs they map to.
const NAMES: [&str; 7] = [
    "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf",
];
const MEMBERS: [&str; 7] = [
    "member-a", "member-b", "member-c", "member-d", "member-e", "member-f", "member-g",
];
// The seven cards of the long list.
const SEVEN: [u32; 7] = [0, 1, 2, 3, 4, 5, 6];

const HOME: [u8; 3] = [255, 0, 255];
const LIST: [u8; 3] = [255, 0, 0];
const DETAIL: [u8; 3] = [0, 0, 160];
const DONE: [u8; 3] = [255, 255, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const BACKGROUND: [u8; 3] = [30, 31, 32];
// A patch colour no card has: its name reads as nothing.
const UNREADABLE: [u8; 3] = [250, 100, 90];

type RunResult = Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>;

// Deterministic pixel noise, the SEL-L6b gate's: a shifted copy of it correlates with it near
// zero.
fn noise(x: u32, y: u32) -> [u8; 3] {
    let mut state = u64::from(y * 64 + x).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x2545_f491;
    let mut next = || {
        state ^= state >> 33;
        state = state.wrapping_mul(0xff51_afd7_ed55_8ccd);
        state ^= state >> 33;
        (state & 0xff) as u8
    };
    [next(), next(), next()]
}

// The red level of card k's patch; the green and blue follow from it.
fn level(card: u32) -> u8 {
    u8::try_from(30 + 36 * card).expect("level")
}

fn patch(card: u32) -> [u8; 3] {
    let red = level(card);
    [red, 220 - red / 2, 90]
}

// One screen: its page's colour; on the list, whether its start and end marks show, the content
// moved up by `offset` pixels, and its cards, each `(card, patch colour)`, card k at content y
// 10 + 12 k.
#[derive(Debug, Clone)]
struct Screen {
    page: [u8; 3],
    start: bool,
    end: bool,
    offset: u32,
    cards: Vec<(u32, [u8; 3])>,
}

fn page(page: [u8; 3]) -> Screen {
    Screen {
        page,
        start: false,
        end: false,
        offset: 0,
        cards: Vec::new(),
    }
}

// The list at content offset `offset` showing `cards`, each with its own patch: its start mark at
// offset 0, its end mark where `end`.
fn list_at(offset: u32, end: bool, cards: &[u32]) -> Screen {
    Screen {
        page: LIST,
        start: offset == 0,
        end,
        offset,
        cards: cards.iter().map(|card| (*card, patch(*card))).collect(),
    }
}

// The list of the four first cards on one screen, both its marks shown, card 1's name unreadable.
fn list_with_an_unread_card() -> Screen {
    let mut screen = list_at(0, true, &[0, 1, 2, 3]);
    screen.cards[1].1 = UNREADABLE;
    screen
}

fn pixel(screen: &Screen, x: u32, y: u32) -> [u8; 3] {
    if y == 0 && x < 4 {
        return match x {
            0 => screen.page,
            1 if screen.start => GREEN,
            2 if screen.end => BLUE,
            3 if screen.page == LIST => GREEN,
            _ => BACKGROUND,
        };
    }
    let content = y + screen.offset;
    for (card, color) in &screen.cards {
        let top = 10 + 12 * card;
        if (top..top + 6).contains(&content) && (4..12).contains(&x) {
            return noise(x - 4, content - top);
        }
        if (top..top + 4).contains(&content) && (12..16).contains(&x) {
            return *color;
        }
    }
    BACKGROUND
}

fn pixels(screen: &Screen) -> Vec<[u8; 3]> {
    (0..HEIGHT)
        .flat_map(|y| (0..WIDTH).map(move |x| pixel(screen, x, y)))
        .collect()
}

// The screen as the scripted device shows it.
fn scripted(screen: &Screen) -> ScriptedFrame {
    ScriptedFrame::rgb(WIDTH, HEIGHT, &pixels(screen))
}

// The screen as a saved frame.
fn saved(screen: &Screen) -> Frame {
    Frame::from_pixels(
        WIDTH,
        HEIGHT,
        pixels(screen).concat(),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("saved frame")
}

// The anchor template: the noise marker, 8 x 6 px.
fn marker_png() -> Vec<u8> {
    let pixels = (0..6)
        .flat_map(|y| (0..8).flat_map(move |x| noise(x, y)))
        .collect::<Vec<u8>>();
    Frame::from_pixels(
        8,
        6,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("marker frame")
    .png_for_artifact()
    .expect("marker PNG")
}

// Reads a card's name from the colour of its patch: the name region starts at the card's anchor,
// and the patch lies 8 px to its right; a colour no card has reads as nothing.
#[derive(Debug)]
struct CardNames;

impl VisionProvider for CardNames {
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
        let x = usize::try_from(request.region.x + 9).expect("x");
        let y = usize::try_from(request.region.y + 1).expect("y");
        let width = usize::try_from(request.frame.width).expect("width");
        let red = request.frame.rgb8_pixels[(y * width + x) * 3];
        let text = (0..7)
            .position(|card| level(card) == red)
            .map_or("", |card| NAMES[card]);
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: text.to_owned(),
            blocks: Vec::new(),
            confidence: Some(0.99),
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

// The cards as a `repeated_anchor` layout on the list page, each naming its member, readable over
// the whole tracking area.
fn card_layout() -> Value {
    let entries = MEMBERS
        .iter()
        .zip(NAMES)
        .map(|(id, name)| json!({"id": id, "aliases": [name]}))
        .collect::<Vec<_>>();
    json!({
        "id": "layout/cards", "page_id": LIST_PAGE, "kind": "repeated_anchor",
        "anchor": "ui/card", "max_instances": 16, "order": "top_to_bottom",
        "instance_rect": {"x": 0, "y": 0, "width": 16, "height": 8},
        "click": {"x": 2, "y": 1, "width": 4, "height": 4},
        "readable_band": {"x": 0, "y": 4, "width": WIDTH, "height": 56},
        "unknown_identity": "reject",
        "features": [{"name": "member", "value": "identity", "target": "card/name",
                      "offset": {"x": 0, "y": 0},
                      "identity": {
                          "entries": entries,
                          "recognition": {"kind": "ocr_aliases", "max_distance": 1,
                                          "minimum_margin": 1,
                                          "minimum_confidence_milli": 800}}}]
    })
}

// The scan of the list: a vertical drag forward of 12 to 48 px, no rewind, the tracking area the
// frame below its first row.
fn scan() -> Value {
    json!({
        "identity_feature": "member",
        "axis": "vertical",
        "advance": {"kind": "drag",
                    "from": {"x": 20, "y": 40, "width": 20, "height": 10},
                    "to": {"x": 20, "y": 14, "width": 20, "height": 10},
                    "duration_ms": 300, "min_content_step_px": 12, "max_content_step_px": 48},
        "reposition": {"kind": "drag",
                       "from": {"x": 20, "y": 50, "width": 20, "height": 10},
                       "to": {"x": 20, "y": 8, "width": 20, "height": 10},
                       "duration_ms": 300, "max_content_step_px": 56},
        "settle": {"interval_ms": 1, "timeout_ms": 1000},
        "track": {"area": {"x": 0, "y": 4, "width": WIDTH, "height": 56},
                  "edge_margin_px": 2,
                  "signature": {"rect": {"x": 8, "y": 0, "width": 4, "height": 4},
                                "columns": 1, "rows": 1},
                  "signature_max_distance_milli": 500,
                  "position_tolerance_px": 1,
                  "min_overlap_instances": 2,
                  "spacing": {"max_axis_gap_px": 20, "columns": 1,
                              "max_trailing_gap_px": 100, "max_cross_lead_px": 10,
                              "max_cross_gap_px": 10}},
        "start": {"check": "check/list_start", "rewind": false},
        "end": {"check": "check/list_end"},
        "bounds": {"max_viewports": 8, "max_reposition_viewports": 4, "max_ms": 120_000}
    })
}

// A trusted tap `id` from `from` to `to`.
fn tap(id: &str, from: &str, to: &str) -> Value {
    json!({"id": id, "from": from, "to": to,
           "click": {"kind": "point", "x": 1, "y": 1},
           "unguarded_trusted_coordinate": true,
           "expect_after": {"page_id": to, "timeout_ms": 1000}})
}

// The reading `id` of the list into `fact_key`, guarded by the list's mark.
fn read_step(id: &str, fact_key: &str) -> Value {
    json!({
        "id": id, "from": "list", "to": "list",
        "guard": {"page_id": "list", "target_id": "guard/list",
                  "expected_rect": {"x": 3, "y": 0, "width": 1, "height": 1},
                  "color_probe": "guard/list"},
        "read_list": {"layout_id": "layout/cards", "key_feature": "member", "key_column": "id",
                      "columns": [], "fact_key": fact_key, "valid_for_ms": VALID_FOR_MS,
                      "scan": scan()}
    })
}

// `read_members`, the reading of the path.
fn read_members() -> Value {
    read_step("read_members", LIST_KEY)
}

// `close`, a tap from the list to done guarded by the list's end mark.
fn close() -> Value {
    json!({"id": "close", "from": "list", "to": "done",
           "click": {"kind": "point", "x": 30, "y": 30},
           "guard": {"page_id": "list", "target_id": "guard/end",
                     "expected_rect": {"x": 2, "y": 0, "width": 1, "height": 1},
                     "color_probe": "guard/end"},
           "expect_after": {"page_id": "done", "timeout_ms": 1000}})
}

// A task from `entry` to `target` over `operations`, its one mapping on `terminal`.
fn task_of(entry: &str, target: &str, terminal: &str, operations: Vec<Value>) -> Value {
    let max_steps = operations.len();
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "entry_page": entry,
        "target_page": target,
        "max_steps": max_steps,
        "scheduling_outcome": {"mappings": [
            {"outcome_key": "listed", "effect": "no_designated_effect",
             "terminal_pages": [terminal]}
        ]},
        "candidate_layouts": [card_layout()],
        "operations": operations
    })
}

// The path: `open`, `read_members` and `close`, from home to done.
fn path_task() -> Value {
    task_of(
        "home",
        "done",
        "done",
        vec![tap("open", "home", "list"), read_members(), close()],
    )
}

// The control of a task of `max_steps` operations in `mode`.
fn control(mode: &str, max_steps: usize) -> Value {
    json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.linear.list",
        "execution_mode": mode,
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": HEIGHT},
        "entry_task_id": "task",
        "capture_interval_ms": 100,
        "step_timeout_ms": 32_000,
        "timeout_ms": 300_000,
        "max_steps": max_steps
    })
}

// The page set: home, list, detail and done, each told by pixel (0, 0).
fn pages() -> Value {
    let pages = ["home", "list", "detail", "done"].map(|page| {
        json!({"id": format!("neutral/{page}"), "required": [format!("page/{page}")],
               "optional": [], "forbidden": []})
    });
    json!({"schema_version": "0.3", "pages": pages})
}

// The recognition pack: the pages' colours, the list's start and end marks, the reading's guard and
// the next step's guard, the card anchor and name, and the card layout.
fn pack() -> Value {
    let colour = |id: &str, x: u32, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "defaults": {"match_metric": "ccoeff_normed", "color_max_distance": 0.0},
        "targets": [
            colour("page/home", 0, HOME),
            colour("page/list", 0, LIST),
            colour("page/detail", 0, DETAIL),
            colour("page/done", 0, DONE),
            colour("check/list_start", 1, GREEN),
            colour("check/list_end", 2, BLUE),
            colour("guard/list", 3, GREEN),
            colour("guard/end", 2, BLUE),
            {"type": "template", "id": "ui/card", "template_path": "assets/card.png",
             "region": "full_frame", "threshold": 0.95},
            {"type": "ocr", "id": "card/name",
             "region": {"x": 0, "y": 0, "width": 12, "height": 4},
             "languages": ["en"], "timeout_ms": 2000, "match_mode": "exact",
             "expected": ["alpha"], "case_sensitive": true, "minimum_confidence": 0.8,
             "model_ref": "PP-OCRv6_medium", "model_sha256": "a".repeat(64)}
        ],
        "candidate_layouts": [card_layout()]
    })
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("JSON bytes")
}

// The package of `task` under a control of `mode` and as many steps as it has operations.
fn package(task: &Value, mode: &str) -> Vec<u8> {
    let steps = task["operations"].as_array().map_or(0, Vec::len);
    let entries = [
        ("control.json", bytes(&control(mode, steps))),
        (
            "resources/manifest.json",
            bytes(&json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        ("resources/operations/task/task.json", bytes(task)),
        ("resources/assets/card.png", marker_png()),
        (
            "resources/recognition/neutral.test.pack.json",
            bytes(&pack()),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            bytes(&pages()),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, content) in entries {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&content).expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

// The package of `task` in `mode` as contained task admission prepares it, or its refusal.
fn load(task: &Value, mode: &str) -> Result<PreparedContainedTask, ContainedTaskError> {
    PreparedContainedTask::load_with_vision_provider(
        "neutral.instance",
        &package(task, mode),
        Arc::new(CardNames),
    )
}

// The linear package of the path, admitted.
fn path_package() -> PreparedContainedTask {
    load(&path_task(), "linear_steps")
        .unwrap_or_else(|error| panic!("the path's package is refused: {error}"))
}

// The step plan of `task` as linear admission reads it, or its refusal.
fn linear_admission(task: &Value) -> Result<linear::LinearPlan, ContainedTaskError> {
    let steps = task["operations"].as_array().map_or(0, Vec::len);
    let control =
        parse_task_control_declaration(control("linear_steps", steps)).expect("a valid control");
    let program: TaskProgram = serde_json::from_value(task.clone()).expect("a task program");
    let page_set: PageSet = serde_json::from_value(pages()).expect("page set");
    let detector = PageDetector::new(page_set).expect("page detector");
    program.validate_linear(&control, &detector)
}

// A run of the path on a device that shows home, then `list` (the list's viewports, one after each
// of the reading's moves), then done.
fn run_path(task: &PreparedContainedTask, list: &[Screen]) -> (RunResult, ScriptedRuntime) {
    let mut screens = vec![vec![scripted(&page(HOME))]];
    screens.extend(list.iter().map(|screen| vec![scripted(screen)]));
    screens.push(vec![scripted(&page(DONE))]);
    let mut device = ScriptedRuntime::new(screens);
    let result = task.run(&mut device);
    (result, device)
}

// The run's normal end on `page` after `executed_steps`, with no key of the kernel's.
fn completed(page: &str, executed_steps: u32) -> ContainedTaskOutcome {
    ContainedTaskOutcome {
        outcome: TaskOutcome::Success,
        final_page: Some(page.to_owned()),
        executed_steps,
        selected_scheduling_outcome: None,
        designated_effect_caps: Vec::new(),
    }
}

fn failed(case: &str, result: RunResult) -> ContainedTaskError {
    match result {
        Err(ContainedTaskRunError::Task(error)) => error,
        Err(other) => panic!("{case}: expected a task failure, got {other:?}"),
        Ok(outcome) => panic!("{case}: expected a task failure, the run ended {outcome:?}"),
    }
}

fn value(error: &ContainedTaskError, key: &str) -> Value {
    error.outcome().value(key).cloned().unwrap_or(Value::Null)
}

// One `ListReading` record of a run: its declaration, its observation time and its list.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ListTrace {
    declaration: ListReadingDeclaration,
    captured_at: SystemTime,
    list: Result<ListReadingRows, ListReadingMiss>,
}

// Every `ListReading` record of the run, in order.
fn lists(device: &ScriptedRuntime) -> Vec<ListTrace> {
    device
        .traces()
        .iter()
        .filter_map(|trace| match trace {
            ContainedTaskTrace::ListReading {
                declaration,
                captured_at,
                list,
            } => Some(ListTrace {
                declaration: declaration.clone(),
                captured_at: *captured_at,
                list: list.clone(),
            }),
            _ => None,
        })
        .collect()
}

// The capture time of every frame a reading's end check passed on (`ListReadingFrame`), in order.
fn end_frames(device: &ScriptedRuntime) -> Vec<SystemTime> {
    device
        .traces()
        .iter()
        .filter_map(|trace| match trace {
            ContainedTaskTrace::ListReadingFrame { captured_at } => Some(*captured_at),
            _ => None,
        })
        .collect()
}

// The step, effect, list and final records of the run, in order, each with its operation (none
// for a record that names none).
fn kinds(device: &ScriptedRuntime) -> Vec<(&'static str, &str)> {
    device
        .traces()
        .iter()
        .filter_map(|trace| match trace {
            ContainedTaskTrace::StepStarted {
                operation_label, ..
            } => Some(("step_started", operation_label.as_str())),
            ContainedTaskTrace::EffectIntent {
                operation_label, ..
            } => Some(("effect_intent", operation_label.as_str())),
            ContainedTaskTrace::ListReadingFrame { .. } => Some(("list_reading_frame", "")),
            ContainedTaskTrace::ListReading { declaration, .. } => {
                Some(("list_reading", declaration.operation_id.as_str()))
            }
            ContainedTaskTrace::StepFinished {
                operation_label, ..
            } => Some(("step_finished", operation_label.as_str())),
            ContainedTaskTrace::Finalizing { .. } => Some(("finalizing", "")),
            _ => None,
        })
        .collect()
}

// Each `StepFinished`: its step index, its operation and its page.
fn finished(device: &ScriptedRuntime) -> Vec<(u32, &str, &str)> {
    device
        .traces()
        .iter()
        .filter_map(|trace| match trace {
            ContainedTaskTrace::StepFinished {
                step_index,
                operation_label,
                page_label,
                ..
            } => Some((*step_index, operation_label.as_str(), page_label.as_str())),
            _ => None,
        })
        .collect()
}

// The declaration `read_members` records with its list.
fn declaration() -> ListReadingDeclaration {
    ListReadingDeclaration {
        operation_id: "read_members".to_owned(),
        fact_key: LIST_KEY.to_owned(),
        valid_for_ms: VALID_FOR_MS,
        frame_indices: Vec::new(),
    }
}

// The row of the business ID `member`.
fn row(member: &str) -> BTreeMap<String, FactScalar> {
    BTreeMap::from([("id".to_owned(), FactScalar::String(member.to_owned()))])
}

// The three viewports of the seven cards: at offsets 0 and 24 without the end mark, at 48 with it.
fn three_viewports() -> [Screen; 3] {
    [
        list_at(0, false, &SEVEN),
        list_at(24, false, &SEVEN),
        list_at(48, true, &SEVEN),
    ]
}

// The list whose second viewport stands 6 px on, outside the advance's window (12 to 48 px) and not
// where the first stood.
fn misaligned_viewports() -> [Screen; 2] {
    [list_at(0, false, &SEVEN), list_at(6, false, &SEVEN)]
}

// The saved frames of a run over the viewports `screens`, as the offline entry takes them: the
// dispatched frame, then every capture of the scan, each viewport settling on two equal captures
// (v0, v0, v1, v1, …).
fn settled(screens: &[Screen]) -> Vec<Frame> {
    screens
        .iter()
        .flat_map(|screen| [saved(screen), saved(screen)])
        .collect()
}

// Test 1 (rulings 2 and 3): the path's reading reads the seven cards over three viewports, its two
// moves its only inputs. The list is whole: the seven rows in ascending byte order of the key, at
// the identities' confidence (990), recorded once with the reading's declaration and observed on
// the frame its end check passed on, as a page-graph run records it (`ListReadingFrame`, then
// `ListReading`), before the step finishes on the list. `close`, whose guard is the list's end mark,
// decides on the reading's last viewport: after the second move the device showed its frame
// exactly twice, the two captures of the last viewport's settle, and `close` pressed with no frame
// of its own. The run is Completed on done after three steps; eight frames in all.
#[test]
fn lk_a_list_is_read_over_three_viewports_and_the_next_step_decides_on_the_last() {
    let task = path_package();
    let (result, device) = run_path(&task, &three_viewports());

    let outcome = result.unwrap_or_else(|error| panic!("the run ends normally, got {error:?}"));
    assert_eq!(outcome, completed(DONE_PAGE, 3));
    let ends = end_frames(&device);
    let [end_frame] = ends.as_slice() else {
        panic!("one end frame: {:?}", kinds(&device));
    };
    assert_eq!(
        lists(&device),
        [ListTrace {
            declaration: declaration(),
            captured_at: *end_frame,
            list: Ok(ListReadingRows {
                rows: MEMBERS.into_iter().map(row).collect(),
                confidence_milli: 990,
            }),
        }]
    );
    assert_eq!(
        kinds(&device),
        [
            ("step_started", "open"),
            ("effect_intent", "open"),
            ("step_finished", "open"),
            ("step_started", "read_members"),
            ("effect_intent", "read_members"),
            ("effect_intent", "read_members"),
            ("list_reading_frame", ""),
            ("list_reading", "read_members"),
            ("step_finished", "read_members"),
            ("step_started", "close"),
            ("effect_intent", "close"),
            ("step_finished", "close"),
            ("finalizing", ""),
        ]
    );
    assert_eq!(
        finished(&device),
        [
            (0, "open", LIST_PAGE),
            (1, "read_members", LIST_PAGE),
            (2, "close", DONE_PAGE),
        ]
    );
    assert_eq!(device.inputs().len(), 4, "{:?}", device.inputs());
    assert_eq!(
        device.captures_after(3),
        2,
        "the last viewport's two frames"
    );
    assert_eq!(device.captures(), 8);
}

// Test 2 (ruling 3): an empty list, both marks on its one viewport, is whole with no row at the
// confidence 1000, observed on that frame, and the reading made no input. A list whose second card
// reads as nothing gives the warning's miss, `list_position_unread` with the count 1, observed on
// its last viewport, the frame its end check passed on, and the run still ends on done. A scan whose
// second viewport is misaligned ends the run with `selection_scan_alignment_rejected`
// (`unexpected_displacement`) at the stage `read_list`: after the reading's one move nothing is
// input (no retry), no frame is captured after its last viewport's settle (no closing decision),
// the reading finishes no step and records no list, and the run does not finalize.
#[test]
fn lk_an_empty_list_an_unread_position_and_a_failed_scan_end_as_list_readings_say() {
    let task = path_package();

    let (result, device) = run_path(&task, &[list_at(0, true, &[])]);
    let outcome = result.unwrap_or_else(|error| panic!("empty: the run ends normally: {error:?}"));
    assert_eq!(outcome, completed(DONE_PAGE, 3));
    let ends = end_frames(&device);
    let [end_frame] = ends.as_slice() else {
        panic!("empty: one end frame: {:?}", kinds(&device));
    };
    assert_eq!(
        lists(&device),
        [ListTrace {
            declaration: declaration(),
            captured_at: *end_frame,
            list: Ok(ListReadingRows {
                rows: Vec::new(),
                confidence_milli: 1000,
            }),
        }]
    );
    assert_eq!(device.inputs().len(), 2, "empty: open's and close's taps");

    let (result, device) = run_path(&task, &[list_with_an_unread_card()]);
    let outcome = result.unwrap_or_else(|error| panic!("unread: the run ends normally: {error:?}"));
    assert_eq!(outcome, completed(DONE_PAGE, 3));
    let ends = end_frames(&device);
    let [end_frame] = ends.as_slice() else {
        panic!("unread: one end frame: {:?}", kinds(&device));
    };
    assert_eq!(
        lists(&device),
        [ListTrace {
            declaration: declaration(),
            captured_at: *end_frame,
            list: Err(ListReadingMiss {
                problem: ResourceReadingProblem::ListPositionUnread,
                count: 1,
            }),
        }]
    );
    assert_eq!(
        finished(&device),
        [
            (0, "open", LIST_PAGE),
            (1, "read_members", LIST_PAGE),
            (2, "close", DONE_PAGE),
        ]
    );

    let (result, device) = run_path(&task, &misaligned_viewports());
    let error = failed("misaligned", result);
    assert_eq!(error.code().as_str(), ALIGNMENT_REJECTED, "{error}");
    assert_eq!(value(&error, "stage"), json!("read_list"));
    assert_eq!(
        value(&error, "scan_alignment_failure"),
        json!("unexpected_displacement")
    );
    assert_eq!(device.inputs().len(), 2, "open's tap and the one move");
    assert_eq!(
        device.captures(),
        5,
        "home, the list's arrival and three settle frames"
    );
    assert!(lists(&device).is_empty());
    assert_eq!(
        kinds(&device),
        [
            ("step_started", "open"),
            ("effect_intent", "open"),
            ("step_finished", "open"),
            ("step_started", "read_members"),
            ("effect_intent", "read_members"),
        ]
    );
}

// Test 3 (ruling 4): a step plan's admission refuses each reading the rulings refuse, at its
// operation, with the existing code: by the reading's member checks (`contained_task_resource_
// reading_invalid`, `<operation id>:<member>`) an optional reading and one with `transition`, a
// `key` or a `select` beside `read_list`, and the 17th reading of a task; by the plan's own rules
// (`contained_task_linear_invalid`) a reading with `when`, one from `any`, one right before an
// optional step, one as the plan's last step, and a task that declares `field_lists`. The path is
// admitted, and as a prerequisite, return-home or startup package it is refused as a reading is
// (`read_list`).
#[test]
fn lk_admission_refuses_each_reading_the_rulings_refuse_at_its_operation() {
    let reading = |patch: &dyn Fn(&mut Value)| {
        let mut step = read_members();
        patch(&mut step);
        task_of(
            "home",
            "done",
            "done",
            vec![tap("open", "home", "list"), step, close()],
        )
    };
    let seventeen = task_of(
        "list",
        "done",
        "done",
        (1..=17)
            .map(|index| {
                read_step(
                    &format!("read_{index:02}"),
                    &format!("inventory.list.example_{index:02}"),
                )
            })
            .chain([close()])
            .collect(),
    );
    let member_cases = [
        (
            "an optional reading",
            reading(&|step| step["optional"] = json!({"settle_ms": 10})),
            "read_members:optional",
        ),
        (
            "an intermediate state",
            reading(&|step| {
                step["transition"] = json!({"kind": "window", "min_ms": 1, "max_ms": 2});
            }),
            "read_members:transition",
        ),
        (
            "a key beside read_list",
            reading(&|step| step["key"] = json!({"key": "back"})),
            "read_members:read_list",
        ),
        (
            "a select beside read_list",
            reading(&|step| {
                step["select"] = json!({"layout_id": "layout/cards",
                                        "policy": {"path": "policies/select.json"}});
            }),
            "read_members:read_list",
        ),
        ("a 17th reading", seventeen, "read_17:operations"),
    ];
    for (case, task, detail) in member_cases {
        let error = match linear_admission(&task) {
            Ok(_) => panic!("{case}: admitted"),
            Err(error) => error,
        };
        assert_eq!(error.code().as_str(), READING_INVALID, "{case}: {error}");
        assert_eq!(error.detail(), Some(detail), "{case}");
    }

    let mut from_any = read_members();
    from_any["from"] = json!("any");
    from_any["to"] = json!("any");
    let mut with_field_lists = path_task();
    with_field_lists["field_lists"] = json!([]);
    let mut skip = tap("skip", "list", "detail");
    skip["optional"] = json!({"settle_ms": 10});
    let plan_cases = [
        (
            "a reading with when",
            reading(&|step| step["when"] = json!({"present": "guard/list"})),
            "reason=when operation=read_members",
        ),
        (
            "a reading from any screen",
            task_of("any", "done", "done", vec![from_any, close()]),
            "reason=any_requires_application operation=read_members",
        ),
        (
            "a reading right before an optional step",
            task_of(
                "home",
                "done",
                "done",
                vec![
                    tap("open", "home", "list"),
                    read_members(),
                    skip,
                    tap("finish", "detail", "done"),
                ],
            ),
            "reason=read_list_before_optional operation=read_members",
        ),
        (
            "a reading as the last step",
            task_of(
                "home",
                "list",
                "list",
                vec![tap("open", "home", "list"), read_members()],
            ),
            "reason=read_list_last operation=read_members",
        ),
        ("field lists", with_field_lists, "reason=field_lists"),
    ];
    for (case, task, detail) in plan_cases {
        let error = match linear_admission(&task) {
            Ok(_) => panic!("{case}: admitted"),
            Err(error) => error,
        };
        assert_eq!(error.code().as_str(), LINEAR_INVALID, "{case}: {error}");
        assert_eq!(error.detail(), Some(detail), "{case}");
    }

    let task = path_package();
    assert!(task.has_list_readings());
    assert_eq!(task.prerequisite_incompatibility(), Some("read_list"));
    assert_eq!(task.startup_incompatibility(), Some("read_list"));
    assert!(!task.is_entry_recovery_compatible());
}

// Test 3, the key sets (ruling 2): a plan that leaves its entry page and comes back to it is cut in
// segments a requested key set may leave out, and none that holds a reading is ever left out. With
// no page read by a mark, the plan list, reading, out to the detail and back loses only its second
// segment, never the reading's own; the plan home, into the list, reading, back home loses nothing.
#[test]
fn lk_a_plan_with_key_sets_never_leaves_its_list_reading_out() {
    let on_entry = task_of(
        "list",
        "list",
        "list",
        vec![
            read_members(),
            tap("open", "list", "detail"),
            tap("back", "detail", "list"),
        ],
    );
    let plan = linear_admission(&on_entry).unwrap_or_else(|error| panic!("on the entry: {error}"));
    assert_eq!(
        plan.segments_without_reads(|_| false),
        BTreeMap::from([(1, 3)])
    );

    let inside = task_of(
        "home",
        "home",
        "home",
        vec![
            tap("open", "home", "list"),
            read_members(),
            tap("leave", "list", "home"),
        ],
    );
    let plan = linear_admission(&inside).unwrap_or_else(|error| panic!("inside: {error}"));
    assert!(plan.segments_without_reads(|_| false).is_empty());
}

// Test 4 (ruling 5): the declaration gate, which admission runs first, refuses a `key` beside
// `read_list` at the reading's pointer (`resource_declaration_invalid`, `InvalidValue` at
// `/operations/1/read_list`), in a page-graph package and in a step plan alike.
#[test]
fn lk_the_declaration_gate_refuses_a_key_beside_read_list_in_both_modes() {
    let mut task = path_task();
    task["operations"][1]["key"] = json!({"key": "back"});
    for mode in ["navigable_route", "linear_steps"] {
        let error = match load(&task, mode) {
            Ok(_) => panic!("{mode}: a key beside read_list is admitted"),
            Err(error) => error,
        };
        assert_eq!(
            error.code().as_str(),
            SharedCode::ResourceDeclarationInvalid.as_str(),
            "{mode}: {error}"
        );
        let issue = error
            .declaration_issue()
            .unwrap_or_else(|| panic!("{mode}: the declaration issue"));
        assert_eq!(issue.field_path, "/operations/1/read_list", "{mode}");
        assert_eq!(
            issue.reason,
            actingcommand_contract::ResourceDeclarationReason::InvalidValue,
            "{mode}"
        );
    }
}

// Test 5 (ruling 6): the offline entry over the saved viewports of a run (the dispatched frame, then
// every capture of the scan: v0, v0, v1, v1, …) answers what the run records on the same
// viewports. The seven cards over three viewports: the same declaration and the same whole list,
// the end check passing on the third viewport only, at content offsets 0, 24 and 48, and no
// warning. The list with an unread card: the same miss, and the warning
// `contained_task_resource_reading_unresolved` with the reading, the key, `list_position_unread` and
// the count 1. The misaligned scan: the run's code, failure and stage. An operation that reads no
// list is refused (`contained_task_resource_reading_invalid`, `close:read_list`).
#[test]
fn lk_the_offline_entry_answers_what_the_run_records_on_the_same_viewports() {
    let task = path_package();

    let viewports = three_viewports();
    let (result, device) = run_path(&task, &viewports);
    let outcome = result.unwrap_or_else(|error| panic!("whole: the run ends normally: {error:?}"));
    assert_eq!(outcome, completed(DONE_PAGE, 3));
    let whole = lists(&device);
    let [recorded] = whole.as_slice() else {
        panic!("whole: one list: {:?}", kinds(&device));
    };
    let offline = dry_run_list_reading(&task, "read_members", settled(&viewports))
        .unwrap_or_else(|error| panic!("whole: the dry run answers: {error}"));
    assert_eq!(offline.declaration, recorded.declaration);
    assert_eq!(offline.list, recorded.list);
    assert_eq!(
        offline
            .viewports
            .iter()
            .map(|viewport| (viewport.content_offset_px, viewport.end_check))
            .collect::<Vec<_>>(),
        [(0, false), (24, false), (48, true)]
    );
    assert!(offline.warning().is_none());

    let unread = [list_with_an_unread_card()];
    let (result, device) = run_path(&task, &unread);
    let outcome = result.unwrap_or_else(|error| panic!("unread: the run ends normally: {error:?}"));
    assert_eq!(outcome, completed(DONE_PAGE, 3));
    let missed = lists(&device);
    let [recorded] = missed.as_slice() else {
        panic!("unread: one list: {:?}", kinds(&device));
    };
    let offline = dry_run_list_reading(&task, "read_members", settled(&unread))
        .unwrap_or_else(|error| panic!("unread: the dry run answers: {error}"));
    assert_eq!(offline.list, recorded.list);
    let warning = offline.warning().expect("the miss's warning");
    assert_eq!(warning.code().as_str(), UNRESOLVED);
    assert_eq!(warning.value("reading_id"), Some(&json!("read_members")));
    assert_eq!(warning.value("fact_key"), Some(&json!(LIST_KEY)));
    assert_eq!(
        warning.value("reading_problem"),
        Some(&json!("list_position_unread"))
    );
    assert_eq!(warning.value("count"), Some(&json!(1)));

    let misaligned = misaligned_viewports();
    let (result, _) = run_path(&task, &misaligned);
    let online = failed("misaligned", result);
    let offline = dry_run_list_reading(&task, "read_members", settled(&misaligned))
        .expect_err("the misaligned scan fails");
    assert_eq!(offline.code().as_str(), online.code().as_str());
    for key in ["stage", "scan_alignment_failure", "viewport_index"] {
        assert_eq!(value(&offline, key), value(&online, key), "{key}");
    }

    let refused =
        dry_run_list_reading(&task, "close", settled(&viewports)).expect_err("close reads no list");
    assert_eq!(refused.code().as_str(), READING_INVALID);
    assert_eq!(refused.detail(), Some("close:read_list"));
}
