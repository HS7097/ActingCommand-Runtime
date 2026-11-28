// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #503 LIST-D (0.12.99; register §1.544; contracts/resource-readings.md, "List readings";
// contracts/candidate-projection.md, "Budgets"; contracts/component-interfaces.md, the `package` row
// 31): a list reading over a layout that declares `provider_evaluations` 32 reads a viewport of
// sixteen cards whose name and number are each read on the card, 32 OCR evaluations, which the
// budget of 16 would refuse; and the kernel's whole admission refuses a member a layout reader does
// not know with the code a Runtime before this slice gives such a package.
//
// Every expected value comes from those rules and the package values below, never from the code.
// The package is LIST-A's synthetic `neutral` step plan (`gate_503_list_a`), admitted by the
// kernel's whole admission (`PreparedContainedTask::load_with_vision_provider`, the declaration
// gate included): `open` (a trusted tap from home into the list), `read_members` (the list reading
// with one integer column, `number`) and `close`, its frame made taller, 64 x 210, for sixteen
// cards. Pixel (0, 0) names the page; on the list pixel (1, 0) is green (its start), pixel (2, 0)
// blue (its end) and pixel (3, 0) green (the reading's guard), so one viewport holds the whole list.
// Card k stands at content y 10 + 12 k: a noise marker (the anchor template, 8 x 6 px) at x 4, a
// 4 x 4 patch of its own colour at x 12 (its signature), and at x 16 a 4 x 4 patch from which a
// scripted OCR provider reads its name (its red level) and below it one from which it reads its
// number; both patches' green level is the read's confidence in hundredths. The offline entry takes
// the settled frames v0, v0. The gate reads no clock.

use super::*;
use actingcommand_contract::{FactScalar, ResourceDeclarationReason};
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    VisionProviderError, VisionProviderErrorCode,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const WIDTH: u32 = 64;
const HEIGHT: u32 = 210;
const CARDS: u32 = 16;
const LIST_KEY: &str = "inventory.list.example_kind_a";
const VALID_FOR_MS: u64 = 86_400_000;
// The name each card shows, down the list.
const NAMES: [&str; 16] = [
    "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliet",
    "kilo", "lima", "mike", "november", "oscar", "papa",
];
// The confidence every patch is read at, in hundredths: above the floor of 800.
const SURE: u8 = 99;

const HOME: [u8; 3] = [255, 0, 255];
const LIST: [u8; 3] = [255, 0, 0];
const DONE: [u8; 3] = [255, 255, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const BACKGROUND: [u8; 3] = [30, 31, 32];

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

// Card `card`'s content top.
fn card_top(card: u32) -> u32 {
    10 + 12 * card
}

// Card `card`'s signature patch: a colour of its own.
fn signature(card: u32) -> [u8; 3] {
    let red = u8::try_from(20 + 14 * card).expect("a red level");
    let green = u8::try_from(200 - 10 * card).expect("a green level");
    [red, green, 90]
}

// The red level card `card`'s name is read from.
fn name_level(card: u32) -> u8 {
    u8::try_from(20 + 6 * card).expect("a name level")
}

// The red level card `card`'s number is read from.
fn number_level(card: u32) -> u8 {
    u8::try_from(130 + 6 * card).expect("a number level")
}

// The number card `card` shows.
fn number(card: u32) -> i64 {
    i64::from(2 * card + 3)
}

// The business ID of the name card `card` shows.
fn member_id(card: u32) -> String {
    format!("member-{card:02}")
}

// The list's one viewport: both its marks shown, the sixteen cards.
fn pixel(x: u32, y: u32) -> [u8; 3] {
    if y == 0 && x < 4 {
        return match x {
            0 => LIST,
            1 | 3 => GREEN,
            _ => BLUE,
        };
    }
    for card in 0..CARDS {
        let top = card_top(card);
        if (top..top + 6).contains(&y) && (4..12).contains(&x) {
            return noise(x - 4, y - top);
        }
        if (top..top + 4).contains(&y) && (12..16).contains(&x) {
            return signature(card);
        }
        if (top..top + 4).contains(&y) && (16..20).contains(&x) {
            return [name_level(card), SURE, 90];
        }
        if (top + 4..top + 8).contains(&y) && (16..20).contains(&x) {
            return [number_level(card), SURE, 90];
        }
    }
    BACKGROUND
}

// The viewport as a saved frame.
fn saved() -> Frame {
    let pixels = (0..HEIGHT)
        .flat_map(|y| (0..WIDTH).map(move |x| pixel(x, y)))
        .collect::<Vec<_>>();
    Frame::from_pixels(
        WIDTH,
        HEIGHT,
        pixels.concat(),
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

// Reads a card's name or number and the read's confidence from the patch 13 px right of the region
// a feature reads: the name region starts at the card's anchor, the number region 4 px below it.
#[derive(Debug)]
struct CardFields;

impl VisionProvider for CardFields {
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
        let text = (0..CARDS)
            .find_map(|card| {
                if name_level(card) == red {
                    Some(NAMES[card as usize].to_owned())
                } else if number_level(card) == red {
                    Some(number(card).to_string())
                } else {
                    None
                }
            })
            .unwrap_or_default();
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text,
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

// The cards as a `repeated_anchor` layout on the list page, each with its `member`, read from its
// name, and its `number`, readable over the frame below its first row, declaring
// `provider_evaluations` when given.
fn card_layout(evaluations: Option<u64>) -> Value {
    let entries = (0..CARDS)
        .map(|card| json!({"id": member_id(card), "aliases": [NAMES[card as usize]]}))
        .collect::<Vec<_>>();
    let mut layout = json!({
        "id": "layout/cards", "page_id": "neutral/list", "kind": "repeated_anchor",
        "anchor": "ui/card", "max_instances": 16, "order": "top_to_bottom",
        "instance_rect": {"x": 0, "y": 0, "width": 16, "height": 8},
        "click": {"x": 2, "y": 1, "width": 4, "height": 4},
        "readable_band": {"x": 0, "y": 4, "width": WIDTH, "height": HEIGHT - 4},
        "unknown_identity": "reject",
        "features": [
            {"name": "member", "value": "identity", "target": "card/name",
             "offset": {"x": 0, "y": 0},
             "identity": {
                 "entries": entries,
                 "recognition": {"kind": "ocr_aliases", "max_distance": 1,
                                 "minimum_margin": 1, "minimum_confidence_milli": 800}}},
            {"name": "number", "value": "ocr_integer", "target": "card/number",
             "offset": {"x": 0, "y": 4},
             "integer": {"min": 0, "max": 999, "minimum_confidence_milli": 800}}
        ]
    });
    if let Some(evaluations) = evaluations {
        layout["provider_evaluations"] = json!(evaluations);
    }
    layout
}

// The scan of the list, LIST-A's: a vertical drag forward of 12 to 48 px, no rewind, the tracking
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
        "track": {"area": {"x": 0, "y": 4, "width": WIDTH, "height": HEIGHT - 4},
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

// `read_members`, the reading of the path: the key `member` and the integer column `number`.
fn read_members() -> Value {
    json!({
        "id": "read_members", "from": "list", "to": "list",
        "guard": {"page_id": "list", "target_id": "guard/list",
                  "expected_rect": {"x": 3, "y": 0, "width": 1, "height": 1},
                  "color_probe": "guard/list"},
        "read_list": {"layout_id": "layout/cards", "key_feature": "member",
                      "key_column": "id",
                      "columns": [{"column": "number", "feature": "number"}],
                      "fact_key": LIST_KEY, "valid_for_ms": VALID_FOR_MS, "scan": scan()}
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

// The path over the layout `layout`: `open`, `read_members` and `close`, from home to done.
fn path_task(layout: &Value) -> Value {
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
        "candidate_layouts": [layout],
        "operations": [tap("open", "home", "list"), read_members(), close()]
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

// The recognition pack of the layout `layout`: the pages' colours, the list's start and end marks,
// the reading's guard and the next step's guard, the card anchor, its name and its number.
fn recognition_pack(layout: &Value) -> Value {
    let colour = |id: &str, x: u32, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    // Each read takes at most 1000 ms, so the reading's sixteen cards of two reads need 32 000 ms,
    // LIST-A's sixteen cards of one read of 2000 ms, within the step's limit.
    let text = |id: &str| {
        json!({"type": "ocr", "id": id,
               "region": {"x": 0, "y": 0, "width": 16, "height": 4},
               "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact",
               "expected": ["alpha"], "case_sensitive": true, "minimum_confidence": 0.1,
               "model_ref": "PP-OCRv6_medium", "model_sha256": "a".repeat(64)})
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
            text("card/name"),
            text("card/number")
        ],
        "candidate_layouts": [layout]
    })
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("JSON bytes")
}

// The package whose task and recognition pack both declare the layout `layout`.
fn package(layout: &Value) -> Vec<u8> {
    let entries = [
        ("control.json", bytes(&control())),
        (
            "resources/manifest.json",
            bytes(&json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        (
            "resources/operations/task/task.json",
            bytes(&path_task(layout)),
        ),
        ("resources/assets/card.png", marker_png()),
        (
            "resources/recognition/neutral.test.pack.json",
            bytes(&recognition_pack(layout)),
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

// The package of the layout `layout` as contained task admission prepares it, or its refusal.
fn load(layout: &Value) -> Result<PreparedContainedTask, ContainedTaskError> {
    PreparedContainedTask::load_with_vision_provider(
        "neutral.instance",
        &package(layout),
        Arc::new(CardFields),
    )
}

// The row of card `card`: its business ID and its number.
fn row(card: u32) -> BTreeMap<String, FactScalar> {
    BTreeMap::from([
        ("id".to_owned(), FactScalar::String(member_id(card))),
        ("number".to_owned(), FactScalar::Integer(number(card))),
    ])
}

// Test 7 (the case it is for): a list reading over the layout of sixteen cards that declares
// `provider_evaluations` 32 reads the one viewport whole: sixteen rows, each with its business ID
// and its number, at the reads' confidence (990), nothing unread and nothing passed over.
#[test]
fn list_d_a_list_reading_reads_the_sixteen_rows_of_a_viewport_with_a_budget_of_32() {
    let prepared = load(&card_layout(Some(32)))
        .unwrap_or_else(|error| panic!("the package with the budget 32 is refused: {error}"));
    let read = dry_run_list_reading(&prepared, "read_members", vec![saved(), saved()])
        .unwrap_or_else(|error| panic!("the dry run answers: {error}"));
    assert_eq!(
        read.list,
        Ok(ListReadingRows {
            rows: (0..CARDS).map(row).collect(),
            confidence_milli: 990,
        })
    );
    assert!(read.unknown.is_empty(), "{:?}", read.unknown);
    assert_eq!(read.declaration.passed_over, None);
}

// Test 1 (ruling 5; the kernel's whole admission): the package whose layout writes
// `provider_evaluations` 32 in its task and its recognition pack is admitted; the same package
// whose layout writes a member no reader of a layout knows is refused before anything runs with
// `resource_declaration_invalid`, `UnknownField` at `/candidate_layouts/0/<member>`: the refusal a
// Runtime before this slice gives the package that writes `provider_evaluations`, a member its
// readers do not know.
#[test]
fn list_d_an_unknown_layout_member_is_refused_as_an_older_runtime_refuses_the_budget() {
    if let Err(error) = load(&card_layout(Some(32))) {
        panic!("the package with the budget 32 is refused: {error}");
    }
    let mut unknown = card_layout(None);
    unknown["evaluation_budget"] = json!(32);
    let error = match load(&unknown) {
        Ok(_) => panic!("a layout member no reader knows is admitted"),
        Err(error) => error,
    };
    assert_eq!(
        error.code().as_str(),
        SharedCode::ResourceDeclarationInvalid.as_str(),
        "{error}"
    );
    let issue = error
        .declaration_issue()
        .unwrap_or_else(|| panic!("the declaration issue: {error}"));
    assert_eq!(issue.field_path, "/candidate_layouts/0/evaluation_budget");
    assert_eq!(issue.reason, ResourceDeclarationReason::UnknownField);
}
