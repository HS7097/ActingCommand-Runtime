// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 K-b and K-c (register §1.453, §1.454; contracts/selection-graph.md, "Declaration",
// "Select over a scan", "Record per path"; contracts/linear-steps.md, "Select steps"; the owner's
// ruling on board #463 that a scrolling choice has two kinds, scrolling to the end of a list whose
// content is not known and straight to a known item, and her option A of question 221; the
// coordinator's rulings in the work order).
//
// K-b: a linear package whose one select step reads its list over a scan runs over a list of more
// than one screen, every move really sent to the device stand-in and every frame the stand-in shows
// answering the moves sent before it. The first kind reads to the end check, decides, returns to the
// chosen card, confirms it on a fresh frame and taps once; the second kind, with `early_stop` under
// the policy the Lab writes for an ordered variable, stops on the screen where its first preference
// appears; a wanted card on no screen ends the first kind normally with its `on_empty` key; every
// move is an effect intent followed by its input and by no completion; a list opened in its middle
// is rewound to its start before it is covered.
//
// K-c: a scan that declares `early_stop` may declare no `end`. It never covers its list: found, the
// card is pressed once the early stop holds; when the list stops moving first, the run fails with a
// code of its own, nothing is pressed and no decision is recorded; the offline entry answers the
// same on the same frames. A scan with neither `end` nor `early_stop`, one without `end` beside
// `on_empty`, and a list reading's scan without `end` are refused, by the declaration gate and by
// admission alike.
//
// Every expected value comes from those lines and the package below, never from the code. The
// package is a synthetic `neutral` package admitted by the kernel's own admission, built on K-R3a's
// card list (`gate_463_r3a`): a 64 x 96 frame whose rows 4 to 55 show the list, card k a noise
// marker (the anchor template, 8 x 6 px) at x 4 and content row 10 + 12 k with a 4 x 4 patch of its
// own colour to its right, which the card reader reads as the card's name. The list holds ten cards,
// content rows 10 to 126 and 6 px below the last, so it scrolls from 0 to 76 px and shows four cards
// at a time. Row 0 holds the page (red on the list, blue on the detail page), the list's start mark
// (green, scrolled to 0), its end mark (blue, scrolled to 76) and the step's guard (green). An
// eleventh name, `kilo`, is on no card. The policy is the one `record stop` writes (#463 S2-6): an
// ordered variable `card` read through `keyed_fact`, a card it ranks kept and scored by its rank
// (weight -1000 milli, so the first preference scores highest), one tie-break key, the lowest
// candidate ID first; the run reads the variable's default, as a run without a dispatch does.
//
// The stand-in shows the list scrolled by its offset: a swipe moves the content by the finger's
// travel (from y1 to y2) within 0 to 76 px, a tap on the list opens the detail page, and every frame
// it gives is committed with a reference of its own, as the host's are. No action seed is drawn, so
// every drag runs between its rectangles' centres: the declared advance moves the list 24 px on, its
// reverse 24 px back, the declared reposition's reverse 36 px back.

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_contract::{
    FrameId, IdentifierIssuer, InputFrameReference, InstanceFactContext, InstanceFactSnapshot,
    ResourceDeclarationIssue, ResourceDeclarationReason, ScanCandidateProjection,
    ScanCoverageState, ScanReposition, ScanStartEvidence, TaskSelectionBlocking,
    TaskSelectionConfirmation, TaskSelectionOutcome, TaskSelectionProjection, TaskSelectionRecord,
    TaskSelectionUnknownCheck, TaskSelectionUpperBound,
};
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    VisionProviderError, VisionProviderErrorCode,
};
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const WIDTH: u32 = 64;
const HEIGHT: u32 = 96;
// The list area: the frame's rows 4 to 55.
const LIST_TOP: u32 = 4;
const LIST_HEIGHT: u32 = 52;
// The cards on the list, top to bottom, and how far the list scrolls: its content runs to 6 px
// below the last card, 132 px, of which the list area shows 52 from row 4.
const CARDS: u32 = 10;
const MAX_OFFSET: u32 = 12 * CARDS - 44;
// The names, the first ten the cards' and the eleventh on no card, and their business IDs.
const NAMES: [&str; 11] = [
    "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliet",
    "kilo",
];
const IDS: [&str; 11] = [
    "card_a", "card_b", "card_c", "card_d", "card_e", "card_f", "card_g", "card_h", "card_i",
    "card_j", "card_k",
];
const CARD_B: &str = "card_b";
const CARD_C: &str = "card_c";
const CARD_E: &str = "card_e";
const CARD_G: &str = "card_g";
// The name on no card.
const CARD_K: &str = "card_k";
const BACKGROUND: [u8; 3] = [30, 31, 32];
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
// The instant the step decides at, and the instance its fact snapshot is of.
const NOW_UNIX_MS: u64 = 1_000_000;
const INSTANCE: &str = "neutral.instance";
// The package's ordered variable and the fact a run reads it as.
const VARIABLE: &str = "card";
const VARIABLE_FACT: &str = "session.dispatch.parameter.card";
// The `on_empty` key, which is the policy's `empty` key, mapped to the list page.
const NOT_FOUND: &str = "card_empty";
const LIST_PAGE: &str = "neutral/list";
const DETAIL_PAGE: &str = "neutral/detail";
// The moves the stand-in is sent: the declared advance (24 px on), its reverse (a corrective, and a
// return's short move toward the start: 24 px back) and the declared reposition's reverse (a
// rewind, and a return's long move toward the start: 36 px back).
const ADVANCE: InputAction = InputAction::Swipe {
    x1: 30,
    y1: 44,
    x2: 30,
    y2: 20,
    duration_ms: 300,
};
const BACK: InputAction = InputAction::Swipe {
    x1: 30,
    y1: 20,
    x2: 30,
    y2: 44,
    duration_ms: 300,
};
const REWIND: InputAction = InputAction::Swipe {
    x1: 30,
    y1: 12,
    x2: 30,
    y2: 48,
    duration_ms: 300,
};
// What nothing unknown gives the unknown-instance check.
const NOTHING_UNKNOWN: TaskSelectionUnknownCheck = TaskSelectionUnknownCheck {
    unknown_instances: 0,
    blocking: TaskSelectionBlocking::None,
    upper_bound: TaskSelectionUpperBound::NoneUnknown,
};

type RunResult = Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>;

// Deterministic pixel noise, K1's: a shifted copy of it correlates with it near zero.
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
fn patch_level(card: u32) -> u8 {
    u8::try_from(20 + 20 * card).expect("level")
}

fn image(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Frame {
    let pixel = &pixel;
    Frame::from_pixels(
        width,
        height,
        (0..height)
            .flat_map(|y| (0..width).flat_map(move |x| pixel(x, y)))
            .collect(),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
}

// What the stand-in shows: the list scrolled by its offset, or the detail page.
#[derive(Clone, Copy)]
enum Shown {
    List(u32),
    Detail,
}

// One pixel of what the stand-in shows.
fn shown_pixel(shown: Shown, x: u32, y: u32) -> [u8; 3] {
    let Shown::List(offset) = shown else {
        return if (x, y) == (0, 0) {
            [0, 0, 255]
        } else {
            BACKGROUND
        };
    };
    if y == 0 && x < 4 {
        return match x {
            0 => [255, 0, 0],
            1 if offset == 0 => [0, 255, 0],
            2 if offset == MAX_OFFSET => [0, 0, 255],
            3 => [0, 255, 0],
            _ => BACKGROUND,
        };
    }
    if !(LIST_TOP..LIST_TOP + LIST_HEIGHT).contains(&y) {
        return BACKGROUND;
    }
    let row = y + offset;
    for card in 0..CARDS {
        let top = 10 + 12 * card;
        if (top..top + 6).contains(&row) && (4..12).contains(&x) {
            return noise(x - 4, row - top);
        }
        if (top..top + 4).contains(&row) && (12..16).contains(&x) {
            let red = patch_level(card);
            return [red, 220 - red / 2, 90];
        }
    }
    BACKGROUND
}

fn screen(shown: Shown) -> Frame {
    image(WIDTH, HEIGHT, |x, y| shown_pixel(shown, x, y))
}

// The frames a forward run over the list scrolled by `offsets` records, each viewport twice as the
// scan's settle takes it: the step's dispatched frame and one capture, then two captures after each
// advance.
fn forward(offsets: &[u32]) -> Vec<Frame> {
    offsets
        .iter()
        .flat_map(|&offset| [screen(Shown::List(offset)), screen(Shown::List(offset))])
        .collect()
}

// The anchor template: the noise marker, 8 x 6 px.
fn marker_png() -> Vec<u8> {
    image(8, 6, noise).png_for_artifact().expect("marker PNG")
}

// Reads a card's name from the colour of its patch, as K1's reader does: the name region starts at
// the card's anchor, and the patch lies 8 px to its right.
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
        let text = (0..CARDS)
            .position(|card| patch_level(card) == red)
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

// The cards as K1's `repeated_anchor` layout, each naming its card, readable in the list area.
fn card_layout() -> Value {
    let entries = IDS
        .iter()
        .zip(NAMES)
        .map(|(id, name)| json!({"id": id, "aliases": [name]}))
        .collect::<Vec<_>>();
    json!({
        "id": "layout/cards", "page_id": LIST_PAGE, "kind": "repeated_anchor",
        "anchor": "ui/card", "max_instances": 16, "order": "top_to_bottom",
        "instance_rect": {"x": 0, "y": 0, "width": 16, "height": 8},
        "click": {"x": 2, "y": 1, "width": 4, "height": 4},
        "readable_band": {"x": 0, "y": LIST_TOP, "width": WIDTH, "height": LIST_HEIGHT},
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

// The scan over the list: its tracking area the list area, an advance of 1 to 44 px (the step
// window a recording's moves give, the list area's 52 less a card's 8), a reposition of up to 52 px,
// a start check that may be rewound to, and, as asked, the end check and the early stop.
fn scan(end: bool, early_stop: bool) -> Value {
    let mut scan = json!({
        "identity_feature": "member",
        "axis": "vertical",
        "advance": {"kind": "drag",
                    "from": {"x": 20, "y": 40, "width": 20, "height": 8},
                    "to": {"x": 20, "y": 16, "width": 20, "height": 8},
                    "duration_ms": 300, "min_content_step_px": 1, "max_content_step_px": 44},
        "reposition": {"kind": "drag",
                       "from": {"x": 20, "y": 44, "width": 20, "height": 8},
                       "to": {"x": 20, "y": 8, "width": 20, "height": 8},
                       "duration_ms": 300, "max_content_step_px": 52},
        "settle": {"interval_ms": 1, "timeout_ms": 1000},
        "track": {"area": {"x": 0, "y": LIST_TOP, "width": WIDTH, "height": LIST_HEIGHT},
                  "edge_margin_px": 2,
                  "signature": {"rect": {"x": 8, "y": 0, "width": 4, "height": 4},
                                "columns": 1, "rows": 1},
                  "signature_max_distance_milli": 500,
                  "position_tolerance_px": 1,
                  "min_overlap_instances": 2,
                  "spacing": {"max_axis_gap_px": 20, "columns": 1,
                              "max_trailing_gap_px": 100, "max_cross_lead_px": 10,
                              "max_cross_gap_px": 10}},
        "start": {"check": "check/list_start", "rewind": true},
        "bounds": {"max_viewports": 16, "max_reposition_viewports": 8, "max_ms": 120_000}
    });
    if end {
        scan["end"] = json!({"check": "check/list_end"});
    }
    if early_stop {
        scan["early_stop"] = json!("upper_bound");
    }
    scan
}

// The first kind of scrolling choice: the scan reads to the end of its list.
fn first_kind() -> Value {
    scan(true, false)
}

// The policy `record stop` writes for a select step (#463 S2-6): the ordered variable `card` read
// through `keyed_fact`, a card it ranks kept and scored by its rank (weight -1000 milli), one it
// does not rank dropped, `none_allowed`, one tie-break key, the lowest candidate ID first.
fn policy() -> Value {
    let rank = json!({"source": "keyed_fact", "fact_key": VARIABLE_FACT, "key_field": "member",
                      "value_column": "rank", "absent": {"type": "integer", "value": 0}});
    json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "pick",
        "applies_to": {
            "candidate_layout_id": "layout/cards",
            "outcome_keys": {"selected": "card_selected", "empty": NOT_FOUND,
                             "insufficient": "card_insufficient",
                             "ambiguous": "card_ambiguous", "unknown": "card_unknown"}
        },
        "fields": [{"name": "member", "value_type": {"type": "enum_string", "allowed": IDS}}],
        "facts": [{"fact_key": VARIABLE_FACT, "max_age_ms": 31_536_000_000_u64,
                   "minimum_confidence_milli": 1000,
                   "value_type": {"type": "record_list", "key_column": "key",
                                  "columns": {"rank": {"type": "integer"}}}}],
        "gates": [{"gate_id": "wanted.card",
                   "predicate": {"kind": "integer_at_least", "value": rank, "threshold": 1},
                   "on_unknown": {"kind": "abort_evaluation"}}],
        "scoring": [{"term_id": "rank.card", "value": rank, "transform": {"kind": "identity"},
                     "weight_milli": -1000, "on_unknown": {"kind": "abort_evaluation"}}],
        "selection": {"mode": "none_allowed", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    })
}

// The step's guard: the green mark of row 0 on the list.
fn guard() -> Value {
    json!({"page_id": "list", "target_id": "guard/list",
           "expected_rect": {"x": 3, "y": 0, "width": 1, "height": 1},
           "color_probe": "guard/list"})
}

// The select step `pick` over the list's scan `scan`, from the list to the detail page; with
// `on_empty`, nobody wanted on the list ends the run on it with `NOT_FOUND`.
fn pick(scan: Value, on_empty: bool) -> Value {
    let mut select = json!({"layout_id": "layout/cards", "policy": {"path": "policies/select.json"},
                            "scan": scan});
    if on_empty {
        select["on_empty"] = json!({"outcome_key": NOT_FOUND});
    }
    json!({
        "id": "pick", "from": "list", "to": "detail",
        "expect_after": {"page_id": "detail", "timeout_ms": 5000, "interval_ms": 200},
        "guard": guard(),
        "select": select
    })
}

fn mapping(key: &str, page: &str) -> Value {
    json!({"outcome_key": key, "effect": "no_designated_effect", "terminal_pages": [page]})
}

fn zip_entries(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, bytes) in entries {
        zip.start_file(*path, options).expect("zip entry");
        zip.write_all(bytes).expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("JSON bytes")
}

// The linear package of the one step `step`, its control declaring the ordered variable `card`
// over every name with the default `wanted`; with `on_empty`, its scheduling outcome maps the list
// to `NOT_FOUND` beside the detail page; with `policy`, the step's document.
fn package_of(step: Value, wanted: &[&str], on_empty: bool, policy: Option<Value>) -> Vec<u8> {
    let options = IDS
        .iter()
        .zip(NAMES)
        .map(|(id, name)| json!({"value": id, "title": {"en": name}}))
        .collect::<Vec<_>>();
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.linear.scan.list",
        "execution_mode": "linear_steps",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": HEIGHT},
        "entry_task_id": "task",
        "capture_interval_ms": 100,
        "step_timeout_ms": 32_000,
        "timeout_ms": 300_000,
        "max_steps": 1,
        "variables": [{"name": VARIABLE, "type": "ordered", "title": {"en": "Card"},
                       "options": options, "min_count": 1, "max_count": IDS.len(),
                       "default": wanted}]
    });
    let mut task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "entry_page": "list",
        "target_page": "detail",
        "max_steps": 1,
        "candidate_layouts": [card_layout()],
        "operations": [step]
    });
    if on_empty {
        task["scheduling_outcome"] = json!({"mappings": [mapping("card_selected", "detail"),
                                                         mapping(NOT_FOUND, "list")]});
    }
    let colour = |id: &str, x: u32, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    let pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "defaults": {"match_metric": "ccoeff_normed", "color_max_distance": 0.0},
        "targets": [
            colour("page/list", 0, [255, 0, 0]),
            colour("page/detail", 0, [0, 0, 255]),
            colour("check/list_start", 1, [0, 255, 0]),
            colour("check/list_end", 2, [0, 0, 255]),
            colour("guard/list", 3, [0, 255, 0]),
            {"type": "template", "id": "ui/card", "template_path": "assets/card.png",
             "region": "full_frame", "threshold": 0.95},
            {"type": "ocr", "id": "card/name",
             "region": {"x": 0, "y": 0, "width": 12, "height": 4},
             "languages": ["en"], "timeout_ms": 2000, "match_mode": "exact",
             "expected": ["alpha"], "case_sensitive": true, "minimum_confidence": 0.8,
             "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256}
        ],
        "candidate_layouts": [card_layout()]
    });
    let pages = ["list", "detail"].map(|page| {
        json!({"id": format!("neutral/{page}"), "required": [format!("page/{page}")],
               "optional": [], "forbidden": []})
    });
    let mut entries = vec![
        ("control.json", bytes(&control)),
        (
            "resources/manifest.json",
            bytes(&json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        ("resources/operations/task/task.json", bytes(&task)),
        ("resources/assets/card.png", marker_png()),
        ("resources/recognition/neutral.test.pack.json", bytes(&pack)),
        (
            "resources/recognition/neutral.test.pages.json",
            bytes(&json!({"schema_version": "0.3", "pages": pages})),
        ),
    ];
    if let Some(policy) = policy {
        entries.push((
            "resources/operations/task/policies/select.json",
            bytes(&policy),
        ));
    }
    zip_entries(&entries)
}

// The package whose one step `pick` selects over `scan`, its variable's default `wanted`.
fn package(scan: Value, wanted: &[&str], on_empty: bool) -> Vec<u8> {
    package_of(pick(scan, on_empty), wanted, on_empty, Some(policy()))
}

// Contained task admission of `bytes`, with the card reader.
fn load(bytes: &[u8]) -> Result<PreparedContainedTask, ContainedTaskError> {
    PreparedContainedTask::load_with_vision_provider(INSTANCE, bytes, Arc::new(CardNames))
}

fn admitted(case: &str, bytes: &[u8]) -> PreparedContainedTask {
    load(bytes).unwrap_or_else(|error| panic!("{case}: refused: {error}"))
}

// The instance's fact snapshot: no facts.
fn snapshot() -> InstanceFactSnapshot {
    InstanceFactSnapshot {
        snapshot_id: "snapshot:test:empty".to_owned(),
        ledger_position: 1,
        context: InstanceFactContext {
            instance_id: INSTANCE.to_owned(),
            server_id: "test".to_owned(),
            game_id: "neutral".to_owned(),
        },
        records: Vec::new(),
    }
}

// What the stand-in saw happen, in order.
#[derive(Debug)]
enum Seen {
    // A frame it gave, committed under this reference.
    Frame(InputFrameReference),
    // An effect intent of this action.
    Intent(InputAction),
    // An input of this action, bound to the frame of this reference.
    Input(InputAction, Option<InputFrameReference>),
    // An effect completion.
    Completed,
}

// The device stand-in (see the header). The kernel's scripted runtime keeps the clock, the
// pacing, the inputs and the traces; the stand-in draws every frame itself.
struct ListDevice {
    clock: ScriptedRuntime,
    shown: Shown,
    // The instance's facts with the package's variables overlaid at their defaults, as the host
    // hands a run without a dispatch.
    snapshot: InstanceFactSnapshot,
    ids: IdentifierIssuer,
    captured: Vec<InputFrameReference>,
    seen: Vec<Seen>,
}

impl ListDevice {
    // The stand-in for a run of `task`, its list scrolled by `offset`.
    fn new(task: &PreparedContainedTask, offset: u32) -> Self {
        let mut snapshot = snapshot();
        task.default_run_values()
            .overlay(&mut snapshot, NOW_UNIX_MS);
        Self {
            clock: ScriptedRuntime::new(vec![vec![ScriptedFrame::rgb(1, 1, &[BACKGROUND])]]),
            shown: Shown::List(offset),
            snapshot,
            ids: IdentifierIssuer::new().expect("identifier issuer"),
            captured: Vec::new(),
            seen: Vec::new(),
        }
    }

    fn inputs(&self) -> &[InputAction] {
        self.clock.inputs()
    }

    // Each decision record, in order.
    fn records(&self) -> Vec<&TaskSelectionRecord> {
        self.clock
            .traces()
            .iter()
            .filter_map(|trace| match trace {
                ContainedTaskTrace::SelectionEvaluated { selection, .. } => {
                    Some(selection.as_ref())
                }
                _ => None,
            })
            .collect()
    }

    // The one decision record.
    fn decision_record(&self, case: &str) -> &TaskSelectionRecord {
        let records = self.records();
        assert_eq!(records.len(), 1, "{case}: one decision record");
        records[0]
    }

    // The last input and the frame it was bound to.
    fn last_input(&self, case: &str) -> (&InputAction, InputFrameReference) {
        self.seen
            .iter()
            .rev()
            .find_map(|event| match event {
                Seen::Input(action, Some(bound)) => Some((action, *bound)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{case}: an input bound to a frame"))
    }

    // Where the frame `frame_id` came among the events.
    fn captured_at(&self, frame_id: FrameId) -> usize {
        self.seen
            .iter()
            .position(
                |event| matches!(event, Seen::Frame(reference) if reference.frame_id == frame_id),
            )
            .expect("a frame the stand-in gave")
    }

    // Where the `n`-th input (from 0) came among the events.
    fn input_at(&self, n: usize) -> usize {
        self.seen
            .iter()
            .enumerate()
            .filter(|(_, event)| matches!(event, Seen::Input(..)))
            .nth(n)
            .map(|(index, _)| index)
            .expect("the input")
    }
}

// The list's offset after a swipe that moves its content by `by` px, within its ends.
fn moved(offset: u32, by: i32) -> u32 {
    let target = i64::from(offset) + i64::from(by);
    u32::try_from(target.clamp(0, i64::from(MAX_OFFSET))).expect("an offset on the list")
}

impl ContainedTaskRuntime for ListDevice {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        let mut observed = ObservedFrame::from(screen(self.shown));
        let frame_id = *self
            .ids
            .mint_frame_id()
            .map_err(|_| "a frame id")?
            .transport();
        let reference = InputFrameReference {
            frame_id,
            width: observed.frame.width,
            height: observed.frame.height,
        };
        observed.input_reference = Some(reference);
        self.captured.push(reference);
        self.seen.push(Seen::Frame(reference));
        Ok(observed)
    }

    fn committed_input_frame(
        &mut self,
        reference: InputFrameReference,
    ) -> Result<Option<InputFrameContext>, Self::Error> {
        Ok(self
            .captured
            .contains(&reference)
            .then(|| InputFrameContext::for_test(reference)))
    }

    fn input(
        &mut self,
        action: InputAction,
        frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.seen.push(Seen::Input(
            action.clone(),
            frame.as_ref().map(InputFrameContext::reference),
        ));
        self.shown = match (self.shown, &action) {
            (Shown::List(offset), InputAction::Swipe { y1, y2, .. }) => {
                Shown::List(moved(offset, *y1 - *y2))
            }
            (Shown::List(_), InputAction::Tap { .. }) => Shown::Detail,
            (shown, _) => shown,
        };
        self.clock.input(action, frame)
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        match &trace {
            ContainedTaskTrace::EffectIntent { action, .. } => {
                self.seen.push(Seen::Intent(action.clone()));
            }
            ContainedTaskTrace::EffectCompleted { .. } => self.seen.push(Seen::Completed),
            _ => {}
        }
        self.clock.record(trace)
    }

    fn sampling_supported(&self) -> bool {
        self.clock.sampling_supported()
    }

    fn step_pacing(&mut self) -> Result<StepPacing, Self::Error> {
        self.clock.step_pacing()
    }

    fn step_action_gap(&mut self) -> Result<Duration, Self::Error> {
        self.clock.step_action_gap()
    }

    fn monotonic_now(&mut self) -> Instant {
        self.clock.monotonic_now()
    }

    fn pause(&mut self, duration: Duration) {
        self.clock.pause(duration);
    }

    fn selection_state(
        &mut self,
        _request: SelectionStateRequest,
    ) -> Result<SelectionState, Self::Error> {
        Ok(SelectionState::Snapshot {
            snapshot: self.snapshot.clone(),
            now_unix_ms: NOW_UNIX_MS,
        })
    }

    fn is_bookkeeping_failure(error: &Self::Error) -> bool {
        <ScriptedRuntime as ContainedTaskRuntime>::is_bookkeeping_failure(error)
    }
}

// A run of `task` on the stand-in, its list scrolled by `offset` when the run starts.
fn run(task: &PreparedContainedTask, offset: u32) -> (RunResult, ListDevice) {
    let mut device = ListDevice::new(task, offset);
    let result = task.run(&mut device);
    (result, device)
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

// The run's normal end on `page` after its one step, with the scheduling outcome `key`.
fn completed(page: &str, key: Option<&str>) -> ContainedTaskOutcome {
    ContainedTaskOutcome {
        outcome: TaskOutcome::Success,
        final_page: Some(page.to_owned()),
        executed_steps: 1,
        selected_scheduling_outcome: key.map(str::to_owned),
        designated_effect_caps: Vec::new(),
    }
}

fn ended(case: &str, result: RunResult) -> ContainedTaskOutcome {
    result.unwrap_or_else(|error| panic!("{case}: expected the run to end normally, got {error:?}"))
}

// The scan's projection a select over a scan records.
fn projection(record: &TaskSelectionRecord) -> &ScanCandidateProjection {
    match &record.projection {
        TaskSelectionProjection::V2(projection) => projection,
        TaskSelectionProjection::V1(_) => panic!("a select over a scan records the v2 projection"),
    }
}

// The content offsets of the coverage viewports.
fn offsets(projection: &ScanCandidateProjection) -> Vec<i32> {
    projection
        .viewports()
        .iter()
        .map(|viewport| viewport.content_offset_px)
        .collect()
}

// Whether `action` is a tap inside the click box of the card whose instance starts at row `top` of
// the confirmation frame: the layout's `click` (2, 1, 4 x 4) placed by the instance at (4, top).
fn tap_in_card(action: &InputAction, top: i32) -> bool {
    matches!(action, InputAction::Tap { x, y }
        if (6..10).contains(x) && (top + 1..top + 5).contains(y))
}

// The confirmation of a select over a scan that confirmed the member `candidate_id` on a frame at
// the content offset `offset`: its frame.
fn confirmed_frame(case: &str, record: &TaskSelectionRecord, member: &str, offset: i32) -> FrameId {
    let TaskSelectionConfirmation::IdentityMatched {
        candidate_id,
        frame_id,
        content_offset_px,
    } = &record.confirmation
    else {
        panic!("{case}: the identity confirmed: {:?}", record.confirmation);
    };
    assert_eq!(candidate_id, member, "{case}");
    assert_eq!(*content_offset_px, offset, "{case}");
    *frame_id
}

// K-b item 4 (and K-c's runs): every input of the run is the effect intent of the same action
// right before it, nothing captured between the two, bound to the frame given last before that
// intent, the frame its move or tap was sampled on. With `tapped`, the step's one effect
// completion comes after its last input, the tap, so no move is followed by a completion; without,
// no completion comes at all.
fn assert_intents_then_inputs(case: &str, device: &ListDevice, tapped: bool) {
    let mut latest: Option<InputFrameReference> = None;
    let mut intended: Option<(&InputAction, Option<InputFrameReference>)> = None;
    let mut inputs = 0_usize;
    let mut completions = Vec::new();
    for event in &device.seen {
        match event {
            Seen::Frame(reference) => {
                assert!(
                    intended.is_none(),
                    "{case}: a frame between an intent and its input"
                );
                latest = Some(*reference);
            }
            Seen::Intent(action) => {
                assert!(intended.is_none(), "{case}: two intents without an input");
                intended = Some((action, latest));
            }
            Seen::Input(action, bound) => {
                let Some((intent, sampled_on)) = intended.take() else {
                    panic!("{case}: input {inputs} without an intent right before it");
                };
                assert_eq!(intent, action, "{case}: input {inputs}");
                assert!(bound.is_some(), "{case}: input {inputs} bound to no frame");
                assert_eq!(*bound, sampled_on, "{case}: input {inputs}");
                inputs += 1;
            }
            Seen::Completed => completions.push(inputs),
        }
    }
    assert!(intended.is_none(), "{case}: an intent without its input");
    assert_eq!(inputs, device.inputs().len(), "{case}");
    if tapped {
        assert_eq!(
            completions,
            [inputs],
            "{case}: one completion, after the tap"
        );
    } else {
        assert!(completions.is_empty(), "{case}: {completions:?}");
    }
}

// K-b 1, the first kind: the variable wants card 1 (`card_b`, content row 22), and the list opens
// at its top. The scan reads to its end, the end check passing at 76 px (coverage `complete`, start
// check on the first viewport, the coverage viewports at 0, 24, 48, 72 and 76), and decides
// `selected` with the one member `layout/cards#00`, nothing unknown. It returns to card 1 with one
// reposition (76 to 40 px) and one reversed advance (40 to 16 px), two return viewports, confirms the
// card on a fresh frame F2 at 16 px, captured after the last move and no coverage viewport, and taps
// once inside card 1's click box on F2 (its instance at row 6), the tap bound to F2. The run is
// Completed on the detail page.
#[test]
fn kb_the_first_kind_reads_to_the_end_returns_confirms_on_a_fresh_frame_and_taps_once() {
    let case = "the first kind";
    let task = admitted(case, &package(first_kind(), &[CARD_B], true));

    let (result, device) = run(&task, 0);

    assert_eq!(ended(case, result), completed(DETAIL_PAGE, None));
    let record = device.decision_record(case);
    assert_eq!(
        record.selection_result,
        TaskSelectionOutcome::Selected { count: 1 }
    );
    assert_eq!(record.selected, ["layout/cards#00"]);
    assert_eq!(record.unknown_check, Some(NOTHING_UNKNOWN));
    let scan = projection(record);
    assert_eq!(scan.coverage().state, ScanCoverageState::Complete);
    assert_eq!(scan.coverage().start, Some(ScanStartEvidence::StartCheck));
    assert_eq!(offsets(scan), [0, 24, 48, 72, 76]);
    assert_eq!(
        scan.reposition(),
        ScanReposition {
            rewind_viewports: 0,
            return_viewports: 2
        }
    );
    assert_eq!(device.inputs().len(), 7, "{:?}", device.inputs());
    assert_eq!(
        device.inputs()[..6],
        [ADVANCE, ADVANCE, ADVANCE, ADVANCE, REWIND, BACK]
    );
    let f2 = confirmed_frame(case, record, "layout/cards#00", 16);
    let (tap, bound) = device.last_input(case);
    assert!(tap_in_card(tap, 6), "{tap:?}");
    assert_eq!(bound.frame_id, f2);
    assert!(device.captured_at(f2) > device.input_at(5));
    assert!(
        scan.viewports()
            .iter()
            .all(|viewport| viewport.frame_id != f2),
        "F2 is no coverage viewport"
    );
}

// K-b 2, the second kind with an end declared: the scan declares `early_stop` and the variable
// ranks card 6 (`card_g`, content row 82) first and card 2 (`card_c`, row 34) second, as the Lab's
// policy reads an ordered variable. Card 2 on the first screen scores below what a card not yet
// seen could (the first preference's score), so the scan goes on; card 6 first appears on the third
// viewport (48 px), where the early stop holds (a tie with the lowest candidate ID first): coverage
// `early_stopped` over 0, 24 and 48, the end never reached, `selected` with `layout/cards#01`
// (card 6, numbered after card 2), nothing unknown. Card 6 is in the band, so the return takes no
// move: the run sends the two advances and then the one tap, inside card 6's click box on F2 at
// 48 px (its instance at row 34), bound to F2. The run is Completed on the detail page.
#[test]
fn kb_the_second_kind_stops_where_its_first_preference_appears() {
    let case = "the second kind";
    let task = admitted(case, &package(scan(true, true), &[CARD_G, CARD_C], false));

    let (result, device) = run(&task, 0);

    assert_eq!(ended(case, result), completed(DETAIL_PAGE, None));
    let record = device.decision_record(case);
    assert_eq!(
        record.selection_result,
        TaskSelectionOutcome::Selected { count: 1 }
    );
    assert_eq!(record.selected, ["layout/cards#01"]);
    assert_eq!(record.unknown_check, Some(NOTHING_UNKNOWN));
    let scan = projection(record);
    assert_eq!(scan.coverage().state, ScanCoverageState::EarlyStopped);
    assert_eq!(scan.coverage().end, None);
    assert_eq!(offsets(scan), [0, 24, 48]);
    assert_eq!(scan.candidates().len(), 2);
    assert_eq!(device.inputs().len(), 3, "{:?}", device.inputs());
    assert_eq!(device.inputs()[..2], [ADVANCE, ADVANCE]);
    let f2 = confirmed_frame(case, record, "layout/cards#01", 48);
    let (tap, bound) = device.last_input(case);
    assert!(tap_in_card(tap, 34), "{tap:?}");
    assert_eq!(bound.frame_id, f2);
}

// K-b 3: the variable wants `card_k`, the name on no card, and the step declares `on_empty`. The
// scan reads to its end (coverage `complete` over 0 to 76 px), every card read and dropped, and
// decides `empty` with nothing unknown: no confirmation, and the run is Completed on the list with
// `NOT_FOUND`. The four advances are the run's only inputs, and nothing completes an effect.
#[test]
fn kb_a_wanted_name_on_no_card_ends_the_first_kind_with_its_on_empty_key() {
    let case = "a wanted name on no card";
    let task = admitted(case, &package(first_kind(), &[CARD_K], true));

    let (result, device) = run(&task, 0);

    assert_eq!(ended(case, result), completed(LIST_PAGE, Some(NOT_FOUND)));
    let record = device.decision_record(case);
    assert_eq!(record.selection_result, TaskSelectionOutcome::Empty);
    assert_eq!(record.outcome_key, NOT_FOUND);
    assert_eq!(record.unknown_check, Some(NOTHING_UNKNOWN));
    assert_eq!(record.confirmation, TaskSelectionConfirmation::NotAttempted);
    let scan = projection(record);
    assert_eq!(scan.coverage().state, ScanCoverageState::Complete);
    assert_eq!(offsets(scan), [0, 24, 48, 72, 76]);
    assert_eq!(
        device.inputs(),
        [ADVANCE, ADVANCE, ADVANCE, ADVANCE].as_slice()
    );
    assert_intents_then_inputs(case, &device, false);
}

// K-b 4: on the first kind from the list's top and from its middle (40 px), together every kind of
// move this fixture makes (a rewind, advances, a return's reposition and its reversed advance; the
// bands overlap, so no corrective is made), each input is the effect intent of the same action
// right before it, bound to the frame given last before that intent, and the step's one effect
// completion comes after the tap.
#[test]
fn kb_every_move_is_an_intent_and_its_input_with_no_completion() {
    let task = admitted("the first kind", &package(first_kind(), &[CARD_B], true));
    for (case, offset, moves) in [
        (
            "from the top",
            0,
            vec![ADVANCE, ADVANCE, ADVANCE, ADVANCE, REWIND, BACK],
        ),
        (
            "from the middle",
            40,
            vec![
                REWIND, REWIND, ADVANCE, ADVANCE, ADVANCE, ADVANCE, REWIND, BACK,
            ],
        ),
    ] {
        let (result, device) = run(&task, offset);

        assert_eq!(ended(case, result), completed(DETAIL_PAGE, None));
        assert_eq!(device.inputs().len(), moves.len() + 1, "{case}");
        assert_eq!(device.inputs()[..moves.len()], moves[..], "{case}");
        assert_intents_then_inputs(case, &device, true);
    }
}

// K-b 5, the cold path: the list opens at 40 px, and the scan declares `start.rewind`. The start
// check fails on the dispatched frame, so the scan rewinds twice (40 to 4 px, 4 to 0 px) until it
// passes, before any advance: the record's start evidence is the rewind check, with two rewind
// viewports; the coverage begins at the list's top, on the frame given after the second rewind, and
// reads to the end as from the top; card 1 is returned to, confirmed at 16 px and tapped once.
#[test]
fn kb_a_list_opened_in_its_middle_is_rewound_to_its_start_before_it_is_covered() {
    let case = "the cold path";
    let task = admitted(case, &package(first_kind(), &[CARD_B], true));

    let (result, device) = run(&task, 40);

    assert_eq!(ended(case, result), completed(DETAIL_PAGE, None));
    assert_eq!(device.inputs()[..3], [REWIND, REWIND, ADVANCE]);
    let record = device.decision_record(case);
    let scan = projection(record);
    assert_eq!(scan.coverage().state, ScanCoverageState::Complete);
    assert_eq!(scan.coverage().start, Some(ScanStartEvidence::RewindCheck));
    assert_eq!(
        scan.reposition(),
        ScanReposition {
            rewind_viewports: 2,
            return_viewports: 2
        }
    );
    assert_eq!(offsets(scan), [0, 24, 48, 72, 76]);
    let first = scan.viewports()[0].frame_id;
    assert!(device.captured_at(first) > device.input_at(1));
    assert!(device.captured_at(first) < device.input_at(2));
    let f2 = confirmed_frame(case, record, "layout/cards#00", 16);
    let (tap, bound) = device.last_input(case);
    assert!(tap_in_card(tap, 6), "{tap:?}");
    assert_eq!(bound.frame_id, f2);
}

// K-c 1: the scan declares `early_stop` and no `end`, and the variable wants card 4 (`card_e`,
// content row 58), first shown on the second screen (24 px). The scan stops there (coverage
// `early_stopped` over 0 and 24, no end evidence) and decides `selected` with `layout/cards#00`,
// nothing unknown; card 4 is in the band, so the return takes no move; it is confirmed on F2 at
// 24 px and tapped once inside its click box (its instance at row 34), bound to F2, after the one
// advance; the step's one completion follows the tap. The run is Completed on the detail page.
#[test]
fn kc_a_scan_without_an_end_finds_its_card_stops_early_and_taps_once() {
    let case = "found without an end";
    let task = admitted(case, &package(scan(false, true), &[CARD_E], false));

    let (result, device) = run(&task, 0);

    assert_eq!(ended(case, result), completed(DETAIL_PAGE, None));
    let record = device.decision_record(case);
    assert_eq!(
        record.selection_result,
        TaskSelectionOutcome::Selected { count: 1 }
    );
    assert_eq!(record.selected, ["layout/cards#00"]);
    assert_eq!(record.unknown_check, Some(NOTHING_UNKNOWN));
    let scan = projection(record);
    assert_eq!(scan.coverage().state, ScanCoverageState::EarlyStopped);
    assert_eq!(scan.coverage().end, None);
    assert_eq!(offsets(scan), [0, 24]);
    assert_eq!(device.inputs().len(), 2, "{:?}", device.inputs());
    assert_eq!(device.inputs()[0], ADVANCE);
    let f2 = confirmed_frame(case, record, "layout/cards#00", 24);
    let (tap, bound) = device.last_input(case);
    assert!(tap_in_card(tap, 34), "{tap:?}");
    assert_eq!(bound.frame_id, f2);
    assert_intents_then_inputs(case, &device, true);
}

// K-c 2: the scan declares `early_stop` and no `end`, and the variable ranks `card_k`, on no card,
// first and card 2 (`card_c`) second. Card 2 is read on the first screen, but a card not yet seen
// could still outrank it, so the early stop never holds; the list moves to its foot (0, 24, 48, 72,
// 76 px) and then twice not at all, and the run fails `selection_scan_early_stop_unreached` at the
// stage `select_scan_end` on the last of those viewports, the seventh (index 6). Card 2 is not
// pressed: the six advances are the run's only inputs, each an intent before it and no completion
// after it, and no decision is recorded.
#[test]
fn kc_a_scan_without_an_end_that_stops_moving_first_fails_and_presses_nothing() {
    let case = "not found without an end";
    let task = admitted(case, &package(scan(false, true), &[CARD_K, CARD_C], false));

    let (result, device) = run(&task, 0);

    let error = failed(case, result);
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::SelectionScanEarlyStopUnreached.as_str(),
        "{error}"
    );
    assert_eq!(value(&error, "stage"), json!("select_scan_end"));
    assert_eq!(value(&error, "viewport_index"), json!(6));
    assert_eq!(
        device.inputs(),
        [ADVANCE, ADVANCE, ADVANCE, ADVANCE, ADVANCE, ADVANCE].as_slice()
    );
    assert!(device.records().is_empty(), "no decision is recorded");
    assert_intents_then_inputs(case, &device, false);
}

// K-c 3: the offline entry the Lab's self-check calls (`dry_run_scan_decide`), given the frames the
// stand-in showed the two runs above, a forward run each viewport twice, answers as the run did:
// found, coverage `early_stopped` over 0 and 24 px, the run's decision (its selected candidate, its
// key, its unknown-instance check) and the chosen member card 4 (content row 58); not found, the
// run's failure with its code, stage and viewport.
#[test]
fn kc_the_offline_entry_answers_as_the_run_does_on_the_same_frames() {
    let found = admitted("found", &package(scan(false, true), &[CARD_E], false));
    let (result, device) = run(&found, 0);
    ended("found", result);
    let record = device.decision_record("found");

    let decided = dry_run_scan_decide(
        &found,
        "pick",
        forward(&[0, 24]),
        (&snapshot(), NOW_UNIX_MS),
        &found.default_run_values(),
        None,
    )
    .unwrap_or_else(|error| panic!("found: expected a decision, got {error}"));

    assert_eq!(decided.scan.coverage, ScanCoverageState::EarlyStopped);
    assert_eq!(
        decided
            .scan
            .viewports
            .iter()
            .map(|viewport| viewport.content_offset_px)
            .collect::<Vec<_>>(),
        [0, 24]
    );
    assert_eq!(decided.decision.selected, record.selected);
    assert_eq!(decided.decision.outcome_key, record.outcome_key);
    assert_eq!(Some(decided.unknown_check), record.unknown_check);
    let chosen = decided.chosen.expect("the chosen card");
    assert_eq!(
        (chosen.identity.as_str(), chosen.content_axis_px),
        (CARD_E, 58)
    );

    let absent = admitted(
        "not found",
        &package(scan(false, true), &[CARD_K, CARD_C], false),
    );
    let (result, _) = run(&absent, 0);
    let online = failed("not found", result);

    let offline = dry_run_scan_decide(
        &absent,
        "pick",
        forward(&[0, 24, 48, 72, 76, 76, 76]),
        (&snapshot(), NOW_UNIX_MS),
        &absent.default_run_values(),
        None,
    )
    .expect_err("not found: the dry run fails as the run did");

    assert_eq!(offline.code().as_str(), online.code().as_str());
    for key in ["stage", "viewport_index"] {
        assert_eq!(value(&offline, key), value(&online, key), "{key}");
    }
}

// The declaration gate's refusal of `bytes` (the check `resource validate`, every parse and
// admission run) and admission's: `resource_declaration_invalid` both, with one and the same issue.
fn refused_alike(case: &str, bytes: &[u8]) -> ResourceDeclarationIssue {
    let bundle =
        ExternallyVerifiedBundle::load_with_vision_provider(INSTANCE, bytes, Arc::new(CardNames))
            .unwrap_or_else(|error| panic!("{case}: the package does not load: {error}"));
    let gate = actingcommand_pack_containment::source::validate_contained_declarations(
        bundle.loaded_bundle(),
    )
    .expect_err("the declaration gate refuses the package");
    assert_eq!(
        gate.code,
        SharedCode::ResourceDeclarationInvalid.as_str(),
        "{case}"
    );
    let issue: ResourceDeclarationIssue =
        serde_json::from_value(gate.details.expect("the gate's issue")).expect("an issue");
    let error = match load(bytes) {
        Ok(_) => panic!("{case}: admitted"),
        Err(error) => error,
    };
    assert_eq!(
        error.code().as_str(),
        SharedCode::ResourceDeclarationInvalid.as_str(),
        "{case}: {error}"
    );
    assert_eq!(error.declaration_issue(), Some(&issue), "{case}");
    issue
}

// K-c 4: a scan that declares neither `end` nor `early_stop` could only fail. The declaration gate
// and admission refuse it alike where a missing `end` is refused, `MissingField` at the step's
// `select/scan/end`.
#[test]
fn kc_a_scan_with_neither_an_end_nor_an_early_stop_is_refused_by_the_gate_and_admission_alike() {
    let issue = refused_alike("neither", &package(scan(false, false), &[CARD_E], false));

    assert_eq!(issue.field_path, "/operations/0/select/scan/end");
    assert_eq!(issue.reason, ResourceDeclarationReason::MissingField);
}

// K-c 5: a scan without `end` never covers its list, so `on_empty` beside it could never take
// effect: the declaration gate and admission refuse it alike, `InvalidValue` at the step's
// `select/on_empty`; the same step with its `end` is admitted.
#[test]
fn kc_a_scan_without_an_end_beside_on_empty_is_refused_and_with_its_end_admitted() {
    admitted("with its end", &package(scan(true, true), &[CARD_E], true));

    let issue = refused_alike(
        "without its end",
        &package(scan(false, true), &[CARD_E], true),
    );

    assert_eq!(issue.field_path, "/operations/0/select/on_empty");
    assert_eq!(issue.reason, ResourceDeclarationReason::InvalidValue);
}

// K-c 6: a list reading reads to its end and has no early stop, so its scan still declares `end`:
// a `read_list` step whose scan declares none is refused as before, by the declaration gate and
// admission alike, `MissingField` at the step's `read_list/scan/end`.
#[test]
fn kc_a_list_readings_scan_without_an_end_is_refused_as_before() {
    let read = json!({
        "id": "read_cards", "from": "list", "to": "list",
        "guard": guard(),
        "read_list": {"layout_id": "layout/cards", "key_feature": "member", "key_column": "id",
                      "columns": [], "fact_key": "inventory.list.example_kind_a",
                      "valid_for_ms": 86_400_000, "scan": scan(false, false)}
    });

    let issue = refused_alike("a list reading", &package_of(read, &[CARD_E], false, None));

    assert_eq!(issue.field_path, "/operations/0/read_list/scan/end");
    assert_eq!(issue.reason, ResourceDeclarationReason::MissingField);
}
