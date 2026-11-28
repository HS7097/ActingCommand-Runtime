// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #503 LIST-B (register §1.536; contracts/resource-readings.md, "List readings", "A list
// of the wanted keys only" and "Offline"; the coordinator's rulings 3 to 9 and Addendum 1 of the
// work order of board #503): a `read_list` over a `repeated_anchor` layout whose key feature is a
// picture identity read from the layout's pool. A card is passed over only when an `other` entry
// of the pool recognises it; a card no picture recognises makes the list unread; a card read as a
// key on one viewport and as an `other` on another is unread; a passed-over card is judged on its
// identity alone, for a picture identity and a text identity alike; the run says how many
// positions it passed over; a reading that passes over a pool without an `other` entry and a
// select step over such a layout are refused.
//
// Every expected value comes from those rulings and the package values below, never from the code.
// The packages are synthetic `neutral` packages admitted by the kernel's whole admission
// (`PreparedContainedTask::load_with_vision_provider`, the declaration gate included), LIST-A's
// path in a step plan: `open`, `read_members` (the key column `id`, the column `count`) and
// `close`. A frame is 64 x 124; pixel (0, 0) names the page, on the list pixel (1, 0) is green at
// its start, pixel (2, 0) blue at its end and pixel (3, 0) green (the reading's guard). Cards stand
// in one column, card k at content y 10 + 12 k, its anchor at x 4: a noise marker of 8 x 6 (the
// anchor template), a 4 x 4 signature of its own colour at x 12, a picture of 6 x 4 at (17, 1) of
// its kind (gray noise of its own, every kind's different), a 4 x 4 patch at x 24 from which a
// scripted OCR provider reads the card's count (its red level less 40, or nothing for the level
// 250) and a 5 x 4 patch at x 28 from which it reads the card's name (for the text identity). The
// keys are `member-a` to `member-i`, pictures of kinds 0 to 8, aliases `alpha` to `india`; the pool
// holds them and the `other` pictures of kinds 9 to 18, scored in the region (12, 0, 8, 6) of each
// card at the threshold 900 and the margin 100. Kind 19 has no picture in the pool. The list
// scrolls 72 px a viewport; the dry run takes each viewport's frame twice (it settles on two equal
// captures).

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_contract::{CandidateUnknownReason, FactScalar};
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::{
    CandidateNameMark, CandidatePictureReading, CandidateUnreadCause, NnProviderRequest,
    NnProviderResult, OcrProviderRequest, OcrProviderResult, VisionProviderError,
    VisionProviderErrorCode,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const WIDTH: u32 = 64;
const HEIGHT: u32 = 124;
const LIST_KEY: &str = "inventory.list.example_kind_a";
const VALID_FOR_MS: u64 = 86_400_000;
const DONE_PAGE: &str = "neutral/done";
const MEMBERS: [&str; 9] = [
    "member-a", "member-b", "member-c", "member-d", "member-e", "member-f", "member-g", "member-h",
    "member-i",
];
// The names the provider reads, by kind: the keys' aliases, then two names far from every alias.
const NAMES: [&str; 11] = [
    "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "zulu",
    "yankee",
];
// The kinds of the pool's `other` pictures, and a kind the pool has no picture of.
const OTHER_KINDS: std::ops::Range<u32> = 9..19;
const UNKNOWN_KIND: u32 = 19;
// The slots of the key cards of the grid of eighty, their keys `member-a` to `member-f`, and their
// counts; `member-g`, `member-h` and `member-i` have no card.
const KEY_SLOTS: [u32; 6] = [3, 17, 30, 44, 61, 77];
const KEY_COUNTS: [i64; 6] = [5, 7, 11, 13, 17, 19];
const SURE: u8 = 99;
const SURE_MILLI: u16 = 990;
const UNREADABLE: u8 = 250;
const THRESHOLD: u16 = 900;
const MARGIN: u16 = 100;
const STEP: u32 = 72;
const NAME_WIDTH: i32 = 5;

const HOME: [u8; 3] = [255, 0, 255];
const LIST: [u8; 3] = [255, 0, 0];
const DONE: [u8; 3] = [255, 255, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const BACKGROUND: [u8; 3] = [30, 31, 32];

// Deterministic pixel noise, LIST-A's marker.
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

// The gray level of the picture of `kind` at (`x`, `y`), 16 to 239.
fn picture(kind: u32, x: u32, y: u32) -> u8 {
    let mut state = (u64::from(kind) + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ (u64::from(y * 8 + x) << 20)
        ^ 0x5bd1_e995;
    state ^= state >> 33;
    state = state.wrapping_mul(0xff51_afd7_ed55_8ccd);
    state ^= state >> 33;
    state = state.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    state ^= state >> 33;
    16 + (state % 224) as u8
}

// Card k's signature: a colour of its own, of distinct levels for every card of the grid.
fn signature(slot: u32) -> [u8; 3] {
    let red = u8::try_from(8 * (slot % 32) + 4).expect("red");
    let green = u8::try_from(60 + 40 * (slot / 32)).expect("green");
    [red, green, 90]
}

// The red level the count `count` is read from; `None` reads nothing.
fn count_level(count: Option<i64>) -> u8 {
    count.map_or(UNREADABLE, |count| {
        u8::try_from(40 + count).expect("count level")
    })
}

// The red level the name of `kind` is read from, for the kinds that have a name.
fn name_level(kind: u32) -> u8 {
    u8::try_from(40 + 20 * kind).unwrap_or(UNREADABLE)
}

// One card: its place in the list, the kind of its picture (and of its name) and its count, `None`
// for a count that does not read.
#[derive(Debug, Clone, Copy)]
struct Card {
    slot: u32,
    kind: u32,
    count: Option<i64>,
}

fn card(slot: u32, kind: u32, count: Option<i64>) -> Card {
    Card { slot, kind, count }
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

fn list_at(offset: u32, end: bool, cards: &[Card]) -> Screen {
    Screen {
        page: LIST,
        start: offset == 0,
        end,
        offset,
        cards: cards.to_vec(),
    }
}

// The list of `cards` over its viewports: offsets 0, 72, 144 and so on, up to the first at which
// the last card lies in the readable band (y 4 to 120), which shows the end mark; each viewport's
// cards are `cards_on(viewport)`.
fn viewports(cards_on: impl Fn(u32) -> Vec<Card>) -> Vec<Screen> {
    let mut screens = Vec::new();
    let mut viewport = 0;
    loop {
        let cards = cards_on(viewport);
        let last = cards.iter().map(|shown| shown.slot).max().unwrap_or(0);
        let offset = STEP * viewport;
        let end = offset + 120 >= 10 + 12 * last + 8;
        screens.push(list_at(offset, end, &cards));
        if end {
            return screens;
        }
        viewport += 1;
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
        if !(top..top + 8).contains(&content) {
            continue;
        }
        let within = content - top;
        match x {
            4..=11 if within < 6 => return noise(x - 4, within),
            12..=15 if within < 4 => return signature(shown.slot),
            17..=22 if (1..5).contains(&within) => {
                let level = picture(shown.kind, x - 17, within - 1);
                return [level; 3];
            }
            24..=27 if within < 4 => return [count_level(shown.count), SURE, 90],
            28..=32 if within < 4 => return [name_level(shown.kind), SURE, 90],
            _ => {}
        }
    }
    BACKGROUND
}

fn pixels(screen: &Screen) -> Vec<[u8; 3]> {
    (0..HEIGHT)
        .flat_map(|y| (0..WIDTH).map(move |x| pixel(screen, x, y)))
        .collect()
}

fn scripted(screen: &Screen) -> ScriptedFrame {
    ScriptedFrame::rgb(WIDTH, HEIGHT, &pixels(screen))
}

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

fn png(width: u32, height: u32, values: Vec<u8>) -> Vec<u8> {
    Frame::from_pixels(
        width,
        height,
        values,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("template frame")
    .png_for_artifact()
    .expect("template PNG")
}

// The anchor template: the noise marker, 8 x 6 px.
fn marker_png() -> Vec<u8> {
    png(
        8,
        6,
        (0..6)
            .flat_map(|y| (0..8).flat_map(move |x| noise(x, y)))
            .collect(),
    )
}

// The picture of `kind`, 6 x 4 px.
fn picture_png(kind: u32) -> Vec<u8> {
    png(
        6,
        4,
        (0..4)
            .flat_map(|y| (0..6).flat_map(move |x| [picture(kind, x, y); 3]))
            .collect(),
    )
}

// Reads a card's count from its count patch (a region 4 wide) and its name from its name patch (a
// region 5 wide), with the confidence of the patch's green level, in hundredths.
#[derive(Debug)]
struct CardReads;

impl VisionProvider for CardReads {
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
        let x = usize::try_from(request.region.x + 1).expect("x");
        let y = usize::try_from(request.region.y + 1).expect("y");
        let width = usize::try_from(request.frame.width).expect("width");
        let at = (y * width + x) * 3;
        let red = request.frame.rgb8_pixels[at];
        let hundredths = request.frame.rgb8_pixels[at + 1];
        let text = if request.region.width == NAME_WIDTH {
            (0_u32..)
                .zip(NAMES)
                .find(|(kind, _)| name_level(*kind) == red)
                .map_or(String::new(), |(_, name)| name.to_owned())
        } else if (40..UNREADABLE).contains(&red) {
            (red - 40).to_string()
        } else {
            "?".to_owned()
        };
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

// How the cards' key is read: from pictures, with or without the pool's `other` entries, or from
// the name patch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Identity {
    Pictures,
    KeysOnly,
    Text,
}

// The pool of the key feature: the keys' pictures and, but for `KeysOnly`, the others'.
fn pool(identity: Identity) -> Value {
    let mut entries = (0_u32..)
        .zip(MEMBERS)
        .map(|(kind, id)| json!({"id": id, "target_id": format!("card/pic_{kind}")}))
        .collect::<Vec<_>>();
    if identity == Identity::Pictures {
        entries.extend(
            OTHER_KINDS.map(|kind| json!({"other": true, "target_id": format!("card/pic_{kind}")})),
        );
    }
    json!({"member": entries})
}

// The cards as a `repeated_anchor` layout on the list page: the key feature `member` read as
// `identity` says, and the count.
fn card_layout(identity: Identity) -> Value {
    let member = match identity {
        Identity::Text => json!({
            "name": "member", "value": "identity", "target": "card/name",
            "offset": {"x": 24, "y": 0},
            "identity": {
                "entries": MEMBERS.iter().zip(NAMES).map(|(id, alias)| json!({"id": id, "aliases": [alias]}))
                    .collect::<Vec<_>>(),
                "recognition": {"kind": "ocr_aliases", "max_distance": 1, "minimum_margin": 1,
                                "minimum_confidence_milli": 800}}
        }),
        Identity::Pictures | Identity::KeysOnly => json!({
            "name": "member", "value": "identity",
            "region": {"x": 12, "y": 0, "width": 8, "height": 6},
            "identity": {
                "entries": MEMBERS.iter().map(|id| json!({"id": id})).collect::<Vec<_>>(),
                "recognition": {"kind": "icon_templates", "minimum_score_milli": THRESHOLD,
                                "minimum_margin_milli": MARGIN}}
        }),
    };
    let mut layout = json!({
        "id": "layout/cards", "page_id": "neutral/list", "kind": "repeated_anchor",
        "anchor": "ui/card", "max_instances": 16, "order": "top_to_bottom",
        "instance_rect": {"x": 0, "y": 0, "width": 30, "height": 8},
        "click": {"x": 2, "y": 1, "width": 4, "height": 4},
        "readable_band": {"x": 0, "y": 4, "width": WIDTH, "height": 116},
        "unknown_identity": "reject",
        "features": [
            member,
            {"name": "count", "value": "ocr_integer", "target": "card/count",
             "offset": {"x": 20, "y": 0},
             "integer": {"min": 0, "max": 9999, "minimum_confidence_milli": 800}}
        ]
    });
    if identity != Identity::Text {
        layout["identity_templates"] = pool(identity);
    }
    layout
}

// The scan of the list: a vertical drag forward of 12 to 100 px, no rewind.
fn scan() -> Value {
    json!({
        "identity_feature": "member",
        "axis": "vertical",
        "advance": {"kind": "drag",
                    "from": {"x": 40, "y": 100, "width": 20, "height": 10},
                    "to": {"x": 40, "y": 10, "width": 20, "height": 10},
                    "duration_ms": 300, "min_content_step_px": 12, "max_content_step_px": 100},
        "reposition": {"kind": "drag",
                       "from": {"x": 40, "y": 104, "width": 20, "height": 10},
                       "to": {"x": 40, "y": 4, "width": 20, "height": 10},
                       "duration_ms": 300, "max_content_step_px": 110},
        "settle": {"interval_ms": 1, "timeout_ms": 1000},
        "track": {"area": {"x": 0, "y": 4, "width": WIDTH, "height": 116},
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
        "bounds": {"max_viewports": 16, "max_reposition_viewports": 4, "max_ms": 120_000}
    })
}

fn tap(id: &str, from: &str, to: &str) -> Value {
    json!({"id": id, "from": from, "to": to,
           "click": {"kind": "point", "x": 1, "y": 1},
           "unguarded_trusted_coordinate": true,
           "expect_after": {"page_id": to, "timeout_ms": 1000}})
}

// `read_members`, passing over.
fn read_members() -> Value {
    json!({
        "id": "read_members", "from": "list", "to": "list",
        "guard": {"page_id": "list", "target_id": "guard/list",
                  "expected_rect": {"x": 3, "y": 0, "width": 1, "height": 1},
                  "color_probe": "guard/list"},
        "read_list": {"layout_id": "layout/cards", "key_feature": "member",
                      "key_column": "id", "columns": [{"column": "count", "feature": "count"}],
                      "fact_key": LIST_KEY, "valid_for_ms": VALID_FOR_MS, "scan": scan(),
                      "others": "pass_over"}
    })
}

fn close() -> Value {
    json!({"id": "close", "from": "list", "to": "done",
           "click": {"kind": "point", "x": 40, "y": 60},
           "guard": {"page_id": "list", "target_id": "guard/end",
                     "expected_rect": {"x": 2, "y": 0, "width": 1, "height": 1},
                     "color_probe": "guard/end"},
           "expect_after": {"page_id": "done", "timeout_ms": 1000}})
}

// The policy `pick` names, over the cards' layout: exactly one card, the highest count first. Fix
// round 1 (the first build): the package's source reads a select step's policy before the kernel
// judges the step, so the package holds a document that fits the layout.
fn pick_policy() -> Value {
    json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "cards",
        "applies_to": {
            "candidate_layout_id": "layout/cards",
            "outcome_keys": {"selected": "card_selected", "empty": "card_empty",
                             "insufficient": "card_insufficient",
                             "ambiguous": "card_ambiguous", "unknown": "card_unknown"}
        },
        "fields": [
            {"name": "member", "value_type": {"type": "enum_string", "allowed": MEMBERS}},
            {"name": "count", "value_type": {"type": "integer"}}
        ],
        "facts": [],
        "gates": [],
        "scoring": [],
        "selection": {"mode": "exactly_one", "required_count": 1},
        "tie_break": [
            {"kind": "value", "value": {"source": "field", "field": "count"},
             "direction": "highest_first"},
            {"kind": "candidate_id", "direction": "lowest_first"}
        ]
    })
}

// A select step over the cards' layout, from the list to done.
fn pick() -> Value {
    json!({"id": "pick", "from": "list", "to": "done",
           "expect_after": {"page_id": "done", "timeout_ms": 1000},
           "guard": {"page_id": "list", "target_id": "guard/end",
                     "expected_rect": {"x": 2, "y": 0, "width": 1, "height": 1},
                     "color_probe": "guard/end"},
           "select": {"layout_id": "layout/cards", "policy": {"path": "policies/select.json"}}})
}

fn path_task(identity: Identity, last: Value) -> Value {
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
        "candidate_layouts": [card_layout(identity)],
        "operations": [tap("open", "home", "list"), read_members(), last]
    })
}

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

fn pages() -> Value {
    let declared = ["home", "list", "done"].map(|name| {
        json!({"id": format!("neutral/{name}"), "required": [format!("page/{name}")],
               "optional": [], "forbidden": []})
    });
    json!({"schema_version": "0.3", "pages": declared})
}

fn ocr(id: &str, width: u32) -> Value {
    json!({"type": "ocr", "id": id,
           "region": {"x": 0, "y": 0, "width": width, "height": 4},
           "languages": ["en"], "timeout_ms": 2000, "match_mode": "exact",
           "expected": ["0"], "case_sensitive": true, "minimum_confidence": 0.1,
           "model_ref": "PP-OCRv6_medium", "model_sha256": "a".repeat(64)})
}

// The recognition pack: the pages' colours, the list's marks, the guards, the card anchor, every
// kind's picture but the unknown kind's, the count and the name, and the card layout.
fn pack(identity: Identity) -> Value {
    let colour = |id: &str, x: u32, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    let mut targets = vec![
        colour("page/home", 0, HOME),
        colour("page/list", 0, LIST),
        colour("page/done", 0, DONE),
        colour("check/list_start", 1, GREEN),
        colour("check/list_end", 2, BLUE),
        colour("guard/list", 3, GREEN),
        colour("guard/end", 2, BLUE),
        json!({"type": "template", "id": "ui/card", "template_path": "assets/card.png",
               "region": "full_frame", "threshold": 0.95}),
        ocr("card/count", 4),
        ocr("card/name", 5),
    ];
    targets.extend((0..UNKNOWN_KIND).map(|kind| {
        json!({"type": "template", "id": format!("card/pic_{kind}"),
               "template_path": format!("assets/pic_{kind}.png"),
               "region": "full_frame", "threshold": 0.9})
    }));
    json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "defaults": {"match_metric": "ccoeff_normed", "color_max_distance": 0.0},
        "targets": targets,
        "candidate_layouts": [card_layout(identity)]
    })
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("JSON bytes")
}

// The package of the path whose key is read as `identity`, ending with `last`, or its refusal.
fn load_with(identity: Identity, last: Value) -> Result<PreparedContainedTask, ContainedTaskError> {
    let selects = last.get("select").is_some();
    let mut entries = vec![
        ("control.json".to_owned(), bytes(&control())),
        (
            "resources/manifest.json".to_owned(),
            bytes(&json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        (
            "resources/operations/task/task.json".to_owned(),
            bytes(&path_task(identity, last)),
        ),
        ("resources/assets/card.png".to_owned(), marker_png()),
        (
            "resources/recognition/neutral.test.pack.json".to_owned(),
            bytes(&pack(identity)),
        ),
        (
            "resources/recognition/neutral.test.pages.json".to_owned(),
            bytes(&pages()),
        ),
    ];
    entries.extend((0..UNKNOWN_KIND).map(|kind| {
        (
            format!("resources/assets/pic_{kind}.png"),
            picture_png(kind),
        )
    }));
    if selects {
        entries.push((
            "resources/operations/task/policies/select.json".to_owned(),
            bytes(&pick_policy()),
        ));
    }
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, content) in entries {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&content).expect("zip bytes");
    }
    let package = zip.finish().expect("finish zip").into_inner();
    PreparedContainedTask::load_with_vision_provider(
        "neutral.instance",
        &package,
        Arc::new(CardReads),
    )
}

// The path whose key is read as `identity`, admitted.
fn admitted(identity: Identity) -> PreparedContainedTask {
    load_with(identity, close()).unwrap_or_else(|error| panic!("the package is refused: {error}"))
}

// The list of `screens` as the dry run reads it for `task`.
fn offline(task: &PreparedContainedTask, screens: &[Screen]) -> ListReadingDryRun {
    let frames = screens
        .iter()
        .flat_map(|screen| [saved(screen), saved(screen)])
        .collect();
    dry_run_list_reading(task, "read_members", frames)
        .unwrap_or_else(|error| panic!("the dry run answers: {error}"))
}

// The row `{id, count}` of the key `member`.
fn row(member: &str, count: i64) -> BTreeMap<String, FactScalar> {
    BTreeMap::from([
        ("id".to_owned(), FactScalar::String(member.to_owned())),
        ("count".to_owned(), FactScalar::Integer(count)),
    ])
}

fn keys(members: &[&str]) -> Vec<String> {
    members.iter().map(|member| (*member).to_owned()).collect()
}

// The grid of eighty cards: the key cards of `KEY_SLOTS` with `KEY_COUNTS`, every other card an
// `other` picture (kind 9 + its slot modulo 10) with a count that reads.
fn grid() -> Vec<Card> {
    (0..80)
        .map(|slot| match KEY_SLOTS.iter().position(|key| *key == slot) {
            Some(index) => card(slot, index as u32, Some(KEY_COUNTS[index])),
            None => card(slot, 9 + slot % 10, Some(i64::from(slot % 40) + 1)),
        })
        .collect()
}

// The one picture read of the position at `index`, on every viewport that read it: each read's
// mark, and its judged sample.
fn picture_reads(
    read: &ListReadingDryRun,
    index: usize,
) -> Vec<(CandidateNameMark, CandidatePictureReading)> {
    read.positions[index]
        .reads
        .iter()
        .map(|position_read| {
            assert!(position_read.samples.is_empty(), "no text was read");
            let [judged] = position_read.pictures.as_slice() else {
                panic!("one judged sample: {position_read:?}");
            };
            (position_read.mark, judged.clone())
        })
        .collect()
}

// Test 1 (rulings 4, 5 and 8; the case the slice exists for): eighty cards over thirteen viewports,
// six of them key cards (`member-a` to `member-f`) and every other one a known `other` picture;
// `member-g`, `member-h` and `member-i` have no card. The list is whole with the six rows and their
// counts, at the counts' confidence 990, 74 positions passed over, nothing unread; the summary
// shows `member-a` to `member-f` and not the other three. The dry run answers each kind of position
// with the mapping's evidence: a key card known by its own picture, its best key that key and its
// picture's target at 990 or more and its best `other` below the threshold; an `other` card passed
// over, none of the keys by its own target at 990 or more, its feature Unknown `out_of_domain`;
// each with the threshold 900 and the margin 100.
#[test]
fn list_b_eighty_cards_over_thirteen_viewports_give_the_six_keys_shown_and_a_whole_list() {
    let screens = viewports(|_| grid());
    assert_eq!(screens.len(), 13, "offsets 0 to 864");
    let read = offline(&admitted(Identity::Pictures), &screens);
    assert_eq!(
        read.list,
        Ok(ListReadingRows {
            rows: MEMBERS[..6]
                .iter()
                .zip(KEY_COUNTS)
                .map(|(member, count)| row(member, count))
                .collect(),
            confidence_milli: SURE_MILLI,
        })
    );
    assert_eq!(read.declaration.passed_over, Some(74));
    assert!(read.unknown.is_empty(), "{:?}", read.unknown);
    assert_eq!(
        read.summary,
        ListDryRunSummary {
            whole: true,
            positions: 80,
            passed_over: 74,
            unread: 0,
            wanted_shown: keys(&MEMBERS[..6]),
            wanted_not_shown: keys(&MEMBERS[6..]),
        }
    );
    for (index, slot) in KEY_SLOTS.iter().enumerate() {
        let position = &read.positions[*slot as usize];
        assert_eq!(
            position.answer,
            ListPositionAnswer::Known {
                key: MEMBERS[index].to_owned()
            },
            "slot {slot}"
        );
        for (mark, judged) in picture_reads(&read, *slot as usize) {
            assert_eq!(mark, CandidateNameMark::Known, "slot {slot}");
            assert_eq!(judged.mark, CandidateNameMark::Known, "slot {slot}");
            assert_eq!(judged.unread_cause, None, "slot {slot}");
            assert_eq!(
                (judged.minimum_score_milli, judged.minimum_margin_milli),
                (THRESHOLD, MARGIN)
            );
            let best = judged.best_key.expect("the best key");
            assert_eq!(
                (best.key, best.target_id),
                (MEMBERS[index].to_owned(), format!("card/pic_{index}")),
                "slot {slot}"
            );
            assert!(best.score_milli >= 990, "slot {slot}: {}", best.score_milli);
            let other = judged.best_other.expect("the best other");
            assert!(other.score_milli < THRESHOLD, "slot {slot}: {other:?}");
        }
    }
    let first_other = &read.positions[0];
    assert_eq!(first_other.answer, ListPositionAnswer::PassedOver);
    for (mark, judged) in picture_reads(&read, 0) {
        assert_eq!(mark, CandidateNameMark::NoneOfTheKeys);
        assert_eq!(judged.mark, CandidateNameMark::NoneOfTheKeys);
        assert_eq!(judged.unread_cause, Some(CandidateUnreadCause::OutOfDomain));
        assert!(
            judged
                .best_key
                .is_some_and(|best| best.score_milli < THRESHOLD)
        );
        let other = judged.best_other.expect("the best other");
        assert_eq!(other.target_id, "card/pic_9");
        assert!(other.score_milli >= 990, "{other:?}");
    }
}

// Test 2 (ruling 4): the same grid, card 50 of a kind the pool has no picture of. No entry reaches
// the threshold, so the card is unread, `identity_unread` with its identity Unknown
// `low_confidence`, and the list is not written (`list_position_unread`, count 1), whatever its
// scores; its read shows the best key and the best `other` both below the threshold. The other 73
// cards of other kinds are passed over.
#[test]
fn list_b_a_card_no_picture_recognises_leaves_the_list_unread() {
    let cards = grid()
        .into_iter()
        .map(|shown| {
            if shown.slot == 50 {
                card(50, UNKNOWN_KIND, Some(3))
            } else {
                shown
            }
        })
        .collect::<Vec<_>>();
    let read = offline(&admitted(Identity::Pictures), &viewports(|_| cards.clone()));
    assert_eq!(
        read.list,
        Err(ListReadingMiss {
            problem: ResourceReadingProblem::ListPositionUnread,
            count: 1,
        })
    );
    assert_eq!(read.declaration.passed_over, Some(73));
    assert_eq!(
        read.positions[50].answer,
        ListPositionAnswer::Unread {
            cause: ListUnreadCause::IdentityUnread,
            identity: None,
            identity_unknown: Some(CandidateUnknownReason::LowConfidence),
        }
    );
    for (mark, judged) in picture_reads(&read, 50) {
        assert_eq!(mark, CandidateNameMark::Unread);
        assert_eq!(judged.mark, CandidateNameMark::Unread);
        assert_eq!(
            judged.unread_cause,
            Some(CandidateUnreadCause::LowConfidence)
        );
        assert!(
            judged
                .best_key
                .is_some_and(|best| best.score_milli < THRESHOLD)
        );
        assert!(
            judged
                .best_other
                .is_some_and(|other| other.score_milli < THRESHOLD)
        );
    }
}

// Test 3 (ruling 5): a card read as a key on one viewport and as an `other` on another. Twelve
// cards over two viewports (offsets 0 and 72); card 7 shows the picture of `member-g` on the first
// and an `other` picture on the second, its count 4 on both, every other card an `other` but card 2
// (`member-a`). The
// list is unread (`list_position_unread`, count 1): card 7 is `partly_passed_over`, keeping
// `member-g`, its reads known on viewport 0 and none of the keys on viewport 1.
#[test]
fn list_b_a_card_read_as_a_key_on_one_viewport_and_as_other_on_another_is_unread() {
    let cards_on = |viewport: u32| {
        (0..12)
            .map(|slot| match slot {
                2 => card(2, 0, Some(5)),
                7 if viewport == 0 => card(7, 6, Some(4)),
                7 => card(7, 16, Some(4)),
                _ => card(slot, 9 + slot % 10, Some(1)),
            })
            .collect::<Vec<_>>()
    };
    let screens = viewports(cards_on);
    assert_eq!(screens.len(), 2);
    let read = offline(&admitted(Identity::Pictures), &screens);
    assert_eq!(
        read.list,
        Err(ListReadingMiss {
            problem: ResourceReadingProblem::ListPositionUnread,
            count: 1,
        })
    );
    assert_eq!(
        read.positions[7].answer,
        ListPositionAnswer::Unread {
            cause: ListUnreadCause::PartlyPassedOver,
            identity: Some(MEMBERS[6].to_owned()),
            identity_unknown: None,
        }
    );
    assert_eq!(
        read.positions[7]
            .reads
            .iter()
            .map(|position_read| (position_read.viewport, position_read.mark))
            .collect::<Vec<_>>(),
        vec![
            (0, CandidateNameMark::Known),
            (1, CandidateNameMark::NoneOfTheKeys)
        ]
    );
}

// Test 4 (ruling 6; Addendum 1, A7): a passed-over card is judged on its identity alone, for a
// picture identity and a text identity alike. On one screen, a key card (`member-a`, count 5), an
// `other` card whose count does not read and an `other` card (count 3): the list is whole with the
// key's row and two positions passed over, the second among them. The same screen with a key card
// (`member-b`) whose count does not read in place of the second card: that key's position is unread
// `feature_unread`, keeping `member-b` (`list_position_unread`, count 1). For the text identity the
// cards' names are `alpha`, `zulu` and `yankee`, and `bravo` for the key whose count does not read.
#[test]
fn list_b_a_passed_over_card_is_judged_on_its_identity_alone_for_pictures_and_for_text() {
    for identity in [Identity::Pictures, Identity::Text] {
        let (other_kind, last_kind) = match identity {
            Identity::Text => (9, 10),
            _ => (11, 12),
        };
        let task = admitted(identity);
        let unreadable_other = list_at(
            0,
            true,
            &[
                card(0, 0, Some(5)),
                card(1, other_kind, None),
                card(2, last_kind, Some(3)),
            ],
        );
        let read = offline(&task, &[unreadable_other]);
        assert_eq!(
            read.list,
            Ok(ListReadingRows {
                rows: vec![row(MEMBERS[0], 5)],
                confidence_milli: SURE_MILLI,
            }),
            "{identity:?}"
        );
        assert_eq!(read.declaration.passed_over, Some(2), "{identity:?}");
        assert_eq!(
            read.positions[1].answer,
            ListPositionAnswer::PassedOver,
            "{identity:?}"
        );

        let unreadable_key = list_at(
            0,
            true,
            &[
                card(0, 0, Some(5)),
                card(1, 1, None),
                card(2, last_kind, Some(3)),
            ],
        );
        let read = offline(&task, &[unreadable_key]);
        assert_eq!(
            read.list,
            Err(ListReadingMiss {
                problem: ResourceReadingProblem::ListPositionUnread,
                count: 1,
            }),
            "{identity:?}"
        );
        assert_eq!(
            read.positions[1].answer,
            ListPositionAnswer::Unread {
                cause: ListUnreadCause::FeatureUnread,
                identity: Some(MEMBERS[1].to_owned()),
                identity_unknown: None,
            },
            "{identity:?}"
        );
    }
}

// Test 5 (ruling 9): the passed-over count is what the run says. A run of the path over one screen
// (`member-a` 5, an `other`, `member-c` 11, an `other`) records its list once, whole, with the
// declaration counting 2 positions passed over, and the dry run over the same screen answers the
// same declaration and list.
#[test]
fn list_b_the_run_says_how_many_positions_it_passed_over() {
    let task = admitted(Identity::Pictures);
    let screen = list_at(
        0,
        true,
        &[
            card(0, 0, Some(5)),
            card(1, 9, Some(2)),
            card(2, 2, Some(11)),
            card(3, 10, Some(3)),
        ],
    );
    let mut device = ScriptedRuntime::new(vec![
        vec![scripted(&page(HOME))],
        vec![scripted(&screen)],
        vec![scripted(&page(DONE))],
    ]);
    let outcome = task
        .run(&mut device)
        .unwrap_or_else(|error| panic!("the run ends normally: {error:?}"));
    assert_eq!(outcome.outcome, TaskOutcome::Success);
    assert_eq!(outcome.final_page.as_deref(), Some(DONE_PAGE));
    let recorded = device
        .traces()
        .iter()
        .filter_map(|trace| match trace {
            ContainedTaskTrace::ListReading {
                declaration, list, ..
            } => Some((declaration.clone(), list.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    let [(declaration, list)] = recorded.as_slice() else {
        panic!("one list: {recorded:?}");
    };
    assert_eq!(declaration.passed_over, Some(2));
    assert_eq!(
        list,
        &Ok(ListReadingRows {
            rows: vec![row(MEMBERS[0], 5), row(MEMBERS[2], 11)],
            confidence_milli: SURE_MILLI,
        })
    );
    let read = offline(&task, &[screen]);
    assert_eq!(&read.declaration, declaration);
    assert_eq!(&read.list, list);
}

// Test 6 (rulings 5 and 7): a reading that declares `others: pass_over` over a picture identity
// whose pool holds no `other` entry is refused at the member (it could never pass over a card):
// `contained_task_resource_reading_invalid`, `read_members:read_list/others`. A select step over a
// `repeated_anchor` layout whose identity is read from pictures is refused at its layout:
// `contained_task_select_invalid`, naming `select/layout_id`.
#[test]
fn list_b_passing_over_a_pool_without_others_and_a_select_over_pictures_are_refused() {
    let refusal = |identity: Identity, last: Value| match load_with(identity, last) {
        Ok(_) => panic!("{identity:?}: admitted"),
        Err(error) => error,
    };
    let error = refusal(Identity::KeysOnly, close());
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::ContainedTaskResourceReadingInvalid.as_str(),
        "{error}"
    );
    assert_eq!(error.detail(), Some("read_members:read_list/others"));

    let error = refusal(Identity::Pictures, pick());
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::ContainedTaskSelectInvalid.as_str(),
        "{error}"
    );
    assert_eq!(
        error.detail(),
        Some("operation=pick select/layout_id layout=layout/cards identity=pictures")
    );
}
