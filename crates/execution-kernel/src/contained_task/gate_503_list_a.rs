// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #503 LIST-A (register §1.533; contracts/resource-readings.md, "List readings", "A list
// of the wanted keys only"; the coordinator's rulings 1 to 5 in the work order of board #503): a
// `read_list` that declares `others: pass_over` passes over every card the identity mapping marks
// none of the wanted keys, so a screen that shows many other cards is read whole, and a wanted key
// no card shows is left out of the list (a pool's `list_rows` counts it 0). A wanted card that is
// not read, and a card known in one viewport and passed over in another, leave the list unread as
// before; the run says how many positions it passed over.
//
// Every expected value comes from those rulings and the package values below, never from the code.
// The packages are synthetic `neutral` packages admitted by the kernel's whole admission
// (`PreparedContainedTask::load_with_vision_provider`, the declaration gate included), S168-LK's
// path in a step plan: `open` (a trusted tap from home into the list), `read_members` (the list
// reading, no columns, the key `inventory.list.example_kind_a`) and `close` (a tap from the list to
// done, guarded by the list's end mark). A frame is 64 x 60. Pixel (0, 0) names the page (home,
// list, done); on the list pixel (1, 0) is green at the list's start, pixel (2, 0) blue at its end
// and pixel (3, 0) green (the reading's guard). Cards stand in one column, card k at content
// y 10 + 12 k: a noise marker (the anchor template, 8 x 6 px) at x 4, a 4 x 4 patch of its own
// colour to its right (the card's signature), and to the right of that a 4 x 4 patch from which a
// scripted OCR provider reads the card's name (its red level) and the read's confidence (its green
// level, in hundredths). The wanted keys are `member-a`, `member-b` and `member-c`, whose aliases
// are `alpha`, `bravo` and `charlie`, read within 1 at a margin of 1 and a floor of 800; `zulu`
// and `yankee` lie further than 2 from every alias, `alpzz` 2 from `alpha`. The runs play scripted
// frames on the kernel's scripted clock; the offline entry takes the settled frames v0, v0, v1, v1.

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_contract::{FactScalar, ScanUnknownCause};
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    VisionProviderError, VisionProviderErrorCode,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const PASSED_OVER: &str = ExecutionKernelCode::ContainedTaskResourceReadingPassedOver.as_str();

const WIDTH: u32 = 64;
const HEIGHT: u32 = 60;
const LIST_KEY: &str = "inventory.list.example_kind_a";
const VALID_FOR_MS: u64 = 86_400_000;
const DONE_PAGE: &str = "neutral/done";
// The names the provider reads, by index; the first three are the wanted keys' aliases.
const NAMES: [&str; 6] = ["alpha", "bravo", "charlie", "zulu", "yankee", "alpzz"];
const MEMBERS: [&str; 3] = ["member-a", "member-b", "member-c"];
const ALPHA: usize = 0;
const BRAVO: usize = 1;
const CHARLIE: usize = 2;
const ZULU: usize = 3;
const YANKEE: usize = 4;
const NEAR_ALPHA: usize = 5;
// The confidences a card's name is read at, in hundredths: above the floor of 800 and below it.
const SURE: u8 = 99;
const UNSURE: u8 = 50;
// The red level of a name patch that reads as no name at all.
const UNREADABLE: u8 = 250;

const HOME: [u8; 3] = [255, 0, 255];
const LIST: [u8; 3] = [255, 0, 0];
const DONE: [u8; 3] = [255, 255, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const BACKGROUND: [u8; 3] = [30, 31, 32];

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

// Card k's signature patch: a colour of its own.
fn signature(slot: u32) -> [u8; 3] {
    let red = u8::try_from(30 + 36 * slot).expect("level");
    [red, 220 - red / 2, 90]
}

// The red level the name `name` (an index of `NAMES`) is read from.
fn name_level(name: usize) -> u8 {
    u8::try_from(40 + 30 * name).expect("name level")
}

// One card: its place in the list, the name its patch reads (none: no name at all) and the read's
// confidence in hundredths.
#[derive(Debug, Clone, Copy)]
struct Card {
    slot: u32,
    name: Option<usize>,
    confidence: u8,
}

fn card(slot: u32, name: usize, confidence: u8) -> Card {
    Card {
        slot,
        name: Some(name),
        confidence,
    }
}

// One screen: its page's colour; on the list, whether its start and end marks show, the content
// moved up by `offset` pixels, and its cards.
#[derive(Debug, Clone)]
struct Screen {
    page: [u8; 3],
    start: bool,
    end: bool,
    offset: u32,
    cards: Vec<Card>,
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

// The list at content offset `offset` showing `cards`: its start mark at offset 0, its end mark
// where `end`.
fn list_at(offset: u32, end: bool, cards: &[Card]) -> Screen {
    Screen {
        page: LIST,
        start: offset == 0,
        end,
        offset,
        cards: cards.to_vec(),
    }
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
    for shown in &screen.cards {
        let top = 10 + 12 * shown.slot;
        if (top..top + 6).contains(&content) && (4..12).contains(&x) {
            return noise(x - 4, content - top);
        }
        if (top..top + 4).contains(&content) && (12..16).contains(&x) {
            return signature(shown.slot);
        }
        if (top..top + 4).contains(&content) && (16..20).contains(&x) {
            let red = shown.name.map_or(UNREADABLE, name_level);
            return [red, shown.confidence, 90];
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

// Reads a card's name and the read's confidence from its name patch: the name region starts at the
// card's anchor, and the name patch lies 12 px to its right; a red level no name has reads as
// nothing.
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
        let x = usize::try_from(request.region.x + 13).expect("x");
        let y = usize::try_from(request.region.y + 1).expect("y");
        let width = usize::try_from(request.frame.width).expect("width");
        let at = (y * width + x) * 3;
        let red = request.frame.rgb8_pixels[at];
        let hundredths = request.frame.rgb8_pixels[at + 1];
        let text = (0..NAMES.len())
            .position(|name| name_level(name) == red)
            .map_or("", |name| NAMES[name]);
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: text.to_owned(),
            blocks: Vec::new(),
            confidence: Some(f32::from(hundredths) / 100.0),
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

// The cards as a `repeated_anchor` layout on the list page, each naming one of the wanted keys or
// none, readable over the whole tracking area.
fn card_layout() -> Value {
    let entries = MEMBERS
        .iter()
        .zip(NAMES)
        .map(|(id, name)| json!({"id": id, "aliases": [name]}))
        .collect::<Vec<_>>();
    json!({
        "id": "layout/cards", "page_id": "neutral/list", "kind": "repeated_anchor",
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

// The scan of the list, S168-LK's: a vertical drag forward of 12 to 48 px, no rewind, the tracking
// area the frame below its first row.
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

// `read_members`, the reading of the path, with `others` when it is given.
fn read_members(others: Option<Value>) -> Value {
    let mut read = json!({"layout_id": "layout/cards", "key_feature": "member",
                          "key_column": "id", "columns": [], "fact_key": LIST_KEY,
                          "valid_for_ms": VALID_FOR_MS, "scan": scan()});
    if let Some(others) = others {
        read["others"] = others;
    }
    json!({
        "id": "read_members", "from": "list", "to": "list",
        "guard": {"page_id": "list", "target_id": "guard/list",
                  "expected_rect": {"x": 3, "y": 0, "width": 1, "height": 1},
                  "color_probe": "guard/list"},
        "read_list": read
    })
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

// The path: `open`, `read_members` (with `others` when it is given) and `close`, from home to
// done.
fn path_task(others: Option<Value>) -> Value {
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "entry_page": "home",
        "target_page": "done",
        "max_steps": 3,
        "scheduling_outcome": {"mappings": [
            {"outcome_key": "listed", "effect": "no_designated_effect",
             "terminal_pages": ["done"]}
        ]},
        "candidate_layouts": [card_layout()],
        "operations": [tap("open", "home", "list"), read_members(others), close()]
    })
}

// The control of the path's step plan.
fn control() -> Value {
    json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.linear.list",
        "execution_mode": "linear_steps",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": HEIGHT},
        "entry_task_id": "task",
        "capture_interval_ms": 100,
        "step_timeout_ms": 32_000,
        "timeout_ms": 300_000,
        "max_steps": 3
    })
}

// The page set: home, list and done, each told by pixel (0, 0).
fn pages() -> Value {
    let pages = ["home", "list", "done"].map(|page| {
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
            colour("page/done", 0, DONE),
            colour("check/list_start", 1, GREEN),
            colour("check/list_end", 2, BLUE),
            colour("guard/list", 3, GREEN),
            colour("guard/end", 2, BLUE),
            {"type": "template", "id": "ui/card", "template_path": "assets/card.png",
             "region": "full_frame", "threshold": 0.95},
            {"type": "ocr", "id": "card/name",
             "region": {"x": 0, "y": 0, "width": 16, "height": 4},
             "languages": ["en"], "timeout_ms": 2000, "match_mode": "exact",
             "expected": ["alpha"], "case_sensitive": true, "minimum_confidence": 0.1,
             "model_ref": "PP-OCRv6_medium", "model_sha256": "a".repeat(64)}
        ],
        "candidate_layouts": [card_layout()]
    })
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("JSON bytes")
}

// The package of `task`.
fn package(task: &Value) -> Vec<u8> {
    let entries = [
        ("control.json", bytes(&control())),
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

// The package of the path with `others` as contained task admission prepares it, or its refusal.
fn load(others: Option<Value>) -> Result<PreparedContainedTask, ContainedTaskError> {
    PreparedContainedTask::load_with_vision_provider(
        "neutral.instance",
        &package(&path_task(others)),
        Arc::new(CardNames),
    )
}

// The path whose reading declares `others: pass_over`, admitted.
fn passing_over() -> PreparedContainedTask {
    load(Some(json!("pass_over")))
        .unwrap_or_else(|error| panic!("the package that passes over is refused: {error}"))
}

// The path whose reading declares no `others`, admitted.
fn reading_every_card() -> PreparedContainedTask {
    load(None).unwrap_or_else(|error| panic!("the package without the member is refused: {error}"))
}

// A run of the path on a device that shows home, then the list's viewports, one after each of the
// reading's moves, then done.
fn run_path(task: &PreparedContainedTask, list: &[Screen]) -> (RunResult, ScriptedRuntime) {
    let mut screens = vec![vec![scripted(&page(HOME))]];
    screens.extend(list.iter().map(|screen| vec![scripted(screen)]));
    screens.push(vec![scripted(&page(DONE))]);
    let mut device = ScriptedRuntime::new(screens);
    let result = task.run(&mut device);
    (result, device)
}

// One `ListReading` record of a run: its declaration and its list.
#[derive(Debug, Clone)]
struct ListTrace {
    declaration: ListReadingDeclaration,
    list: Result<ListReadingRows, ListReadingMiss>,
}

// Every `ListReading` record of the run, in order.
fn lists(device: &ScriptedRuntime) -> Vec<ListTrace> {
    device
        .traces()
        .iter()
        .filter_map(|trace| match trace {
            ContainedTaskTrace::ListReading {
                declaration, list, ..
            } => Some(ListTrace {
                declaration: declaration.clone(),
                list: list.clone(),
            }),
            _ => None,
        })
        .collect()
}

// The saved frames of a run over the viewports `screens`, as the offline entry takes them: the
// dispatched frame, then every capture of the scan, each viewport settling on two equal captures.
fn settled(screens: &[Screen]) -> Vec<Frame> {
    screens
        .iter()
        .flat_map(|screen| [saved(screen), saved(screen)])
        .collect()
}

// The list of `screens` as the offline entry reads it for `task`.
fn offline(task: &PreparedContainedTask, screens: &[Screen]) -> ListReadingDryRun {
    dry_run_list_reading(task, "read_members", settled(screens))
        .unwrap_or_else(|error| panic!("the dry run answers: {error}"))
}

// The row of the business ID `member`.
fn row(member: &str) -> BTreeMap<String, FactScalar> {
    BTreeMap::from([("id".to_owned(), FactScalar::String(member.to_owned()))])
}

// The miss `list_position_unread` of `count` positions.
fn unread(count: u32) -> Result<ListReadingRows, ListReadingMiss> {
    Err(ListReadingMiss {
        problem: ResourceReadingProblem::ListPositionUnread,
        count,
    })
}

// One screen of the list, both its marks shown: `alpha`, `zulu`, `charlie` and `yankee`, each read
// with confidence.
fn mixed() -> Screen {
    list_at(
        0,
        true,
        &[
            card(0, ALPHA, SURE),
            card(1, ZULU, SURE),
            card(2, CHARLIE, SURE),
            card(3, YANKEE, SURE),
        ],
    )
}

// Test 1 (rulings 1 to 3): wanted and other cards mixed on one screen. A reading that passes over
// holds the wanted keys only (`member-a`, `member-c`), whole, at the identities' confidence (990),
// and passes over 2 positions; it says so beside nothing unknown. The same package without the
// member reads every card, so the two other cards make the list unread (`list_position_unread`,
// count 2), as before, and nothing is passed over.
#[test]
fn list_a_wanted_and_other_cards_mixed_give_the_wanted_keys_only_and_a_whole_list() {
    let read = offline(&passing_over(), &[mixed()]);
    assert_eq!(
        read.list,
        Ok(ListReadingRows {
            rows: vec![row(MEMBERS[0]), row(MEMBERS[2])],
            confidence_milli: 990,
        })
    );
    assert_eq!(read.declaration.passed_over, Some(2));
    assert!(read.unknown.is_empty(), "{:?}", read.unknown);
    assert!(read.warning().is_none());

    let before = offline(&reading_every_card(), &[mixed()]);
    assert_eq!(before.list, unread(2), "without the member, as before");
    assert_eq!(before.declaration.passed_over, None);
}

// Test 2 (ruling 2): a wanted card read below the floor (`bravo` at 500) and a card near a wanted
// name without matching it (`alpzz`, 2 from `alpha`) are unread, so the list is not read whole and
// nothing is written (`list_position_unread`, count 2), while the far card beside them (`zulu`) is
// passed over. A card whose name reads as nothing is unread too.
#[test]
fn list_a_a_wanted_card_read_low_or_near_leaves_the_list_unread() {
    let task = passing_over();
    let low_and_near = list_at(
        0,
        true,
        &[
            card(0, ALPHA, SURE),
            card(1, BRAVO, UNSURE),
            card(2, NEAR_ALPHA, SURE),
            card(3, ZULU, SURE),
        ],
    );
    let read = offline(&task, &[low_and_near]);
    assert_eq!(read.list, unread(2));
    assert_eq!(read.declaration.passed_over, Some(1));
    let warning = read.warning().expect("the miss's warning");
    assert_eq!(warning.value("count"), Some(&json!(2)));

    let empty = list_at(
        0,
        true,
        &[
            card(0, ZULU, SURE),
            Card {
                slot: 1,
                name: None,
                confidence: SURE,
            },
        ],
    );
    let read = offline(&task, &[empty]);
    assert_eq!(read.list, unread(1), "a name that reads as nothing");
    assert_eq!(read.declaration.passed_over, Some(1));
}

// Test 3 (ruling 3): no wanted card on any page. Every card is passed over and the list is whole
// with no row, at the confidence 1000 an empty list carries, where today's rules write the empty
// list; a pool that reads its rows counts each wanted key 0.
#[test]
fn list_a_no_wanted_card_writes_the_empty_list() {
    let none_wanted = list_at(0, true, &[card(0, ZULU, SURE), card(1, YANKEE, SURE)]);
    let read = offline(&passing_over(), &[none_wanted]);
    assert_eq!(
        read.list,
        Ok(ListReadingRows {
            rows: Vec::new(),
            confidence_milli: 1000,
        })
    );
    assert_eq!(read.declaration.passed_over, Some(2));
}

// Test 4 (ruling 2): one position known in one viewport and passed over in another. Over two
// viewports (offsets 0 and 24, the end mark on the second), card 2 reads `charlie` on the first and
// `zulu` on the second; the cards after it are passed over on every read. The list is unread
// (`list_position_unread`, count 1, no `selection_scan_identity_mismatch`: the second read names no
// key), and the one position the offline entry could not read is card 2, first read on viewport 0,
// `contradicted`, keeping the key it read.
#[test]
fn list_a_a_position_known_in_one_viewport_and_passed_over_in_another_is_unread() {
    let cards = |second: usize| {
        [
            card(0, ALPHA, SURE),
            card(1, BRAVO, SURE),
            card(2, second, SURE),
            card(3, YANKEE, SURE),
            card(4, ZULU, SURE),
            card(5, YANKEE, SURE),
        ]
    };
    let viewports = [
        list_at(0, false, &cards(CHARLIE)),
        list_at(24, true, &cards(ZULU)),
    ];
    let read = offline(&passing_over(), &viewports);
    assert_eq!(read.list, unread(1));
    assert_eq!(read.declaration.passed_over, Some(3));
    let [position] = read.unknown.as_slice() else {
        panic!("one unknown position: {:?}", read.unknown);
    };
    assert_eq!(
        (
            position.position_index,
            position.first_read_viewport,
            position.cause,
            position.identity.as_deref()
        ),
        (2, 0, ScanUnknownCause::Contradicted, Some(MEMBERS[2]))
    );
}

// Test 5 (ruling 4): the passed-over count is what the run says. A run of the path over the mixed
// screen records its list once, whole, with the declaration counting 2 positions passed over, and
// the offline entry over the same screen answers the same declaration and list. The note the host
// writes for it is `contained_task_resource_reading_passed_over` with the reading, the key and the
// count 2, and no card's text. The same run without the member records the miss of the two other
// cards, a declaration that counts nothing and no note.
#[test]
fn list_a_the_run_says_how_many_positions_it_passed_over() {
    let task = passing_over();
    let (result, device) = run_path(&task, &[mixed()]);
    let outcome = result.unwrap_or_else(|error| panic!("the run ends normally: {error:?}"));
    assert_eq!(outcome.outcome, TaskOutcome::Success);
    assert_eq!(outcome.final_page.as_deref(), Some(DONE_PAGE));
    let recorded = lists(&device);
    let [list] = recorded.as_slice() else {
        panic!("one list: {recorded:?}");
    };
    assert_eq!(list.declaration.passed_over, Some(2));
    assert_eq!(
        list.list,
        Ok(ListReadingRows {
            rows: vec![row(MEMBERS[0]), row(MEMBERS[2])],
            confidence_milli: 990,
        })
    );
    let read = offline(&task, &[mixed()]);
    assert_eq!(read.declaration, list.declaration);
    assert_eq!(read.list, list.list);
    let note = list
        .declaration
        .passed_over_outcome()
        .expect("the note of a list that passes over");
    assert_eq!(note.code().as_str(), PASSED_OVER);
    assert_eq!(note.value("reading_id"), Some(&json!("read_members")));
    assert_eq!(note.value("fact_key"), Some(&json!(LIST_KEY)));
    assert_eq!(note.value("count"), Some(&json!(2)));
    for name in NAMES {
        assert!(
            !format!("{note:?}").contains(name),
            "the note names no card: {note:?}"
        );
    }

    let (result, device) = run_path(&reading_every_card(), &[mixed()]);
    let outcome =
        result.unwrap_or_else(|error| panic!("without: the run ends normally: {error:?}"));
    assert_eq!(outcome.outcome, TaskOutcome::Success);
    let recorded = lists(&device);
    let [list] = recorded.as_slice() else {
        panic!("without: one list: {recorded:?}");
    };
    assert_eq!(list.list, unread(2));
    assert_eq!(list.declaration.passed_over, None);
    assert!(list.declaration.passed_over_outcome().is_none());
}

// Test 6 (ruling 1, the closed vocabulary): the declaration gate, which admission runs first, takes
// `others` only as the word `pass_over`: another word is `InvalidValue` and a number
// `InvalidType`, each at `/operations/1/read_list/others` (`resource_declaration_invalid`); the
// word and a `null` (an absent member) are admitted.
#[test]
fn list_a_the_declaration_gate_takes_others_only_as_pass_over() {
    for (others, reason) in [
        (
            json!("keep"),
            actingcommand_contract::ResourceDeclarationReason::InvalidValue,
        ),
        (
            json!(7),
            actingcommand_contract::ResourceDeclarationReason::InvalidType,
        ),
    ] {
        let error = match load(Some(others.clone())) {
            Ok(_) => panic!("{others}: admitted"),
            Err(error) => error,
        };
        assert_eq!(
            error.code().as_str(),
            SharedCode::ResourceDeclarationInvalid.as_str(),
            "{others}: {error}"
        );
        let issue = error
            .declaration_issue()
            .unwrap_or_else(|| panic!("{others}: the declaration issue"));
        assert_eq!(
            issue.field_path, "/operations/1/read_list/others",
            "{others}"
        );
        assert_eq!(issue.reason, reason, "{others}");
    }
    passing_over();
    load(Some(Value::Null)).unwrap_or_else(|error| panic!("a null others is refused: {error}"));
}
