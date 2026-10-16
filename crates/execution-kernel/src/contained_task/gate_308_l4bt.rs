// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #308 slice SEL-L4b-T (the SEL L4b-T model row of 2026-10-17, tests L4bT-T1 to L4bT-T10;
// forced-compromises mapping R-07; wave-17 acceptance ruling 1): an `expect_identity` confirms the
// business ID its `choice_of` select step chose through a pool of template targets in one identity
// region, with the text path's code and states. On each destination frame every pool entry passes
// by its own target's threshold; exactly one business ID among the passing entries is a reading,
// none is `absent` and two or more are `unknown`. Gate exec 82.
//
// Every package is a synthetic `neutral` package in a 400 x 600 frame, admitted through
// `PreparedContainedTask::load_with_vision_provider`: pixel (0, 0) names the page, pixel (3, 0) is
// the select step's guard on the list. The list holds three fixed slots 120 px apart; each shows
// the 12 x 12 noise image of its member (an icon), a name code read by a scripted OCR provider and
// an open mark. The destination page shows up to two of those images in its identity region
// (x 100, y 300, 60 x 30), and its pool holds one template target of each member there, plus a
// second image of `member.b`. The runs play scripted frames on the scripted clock (EXEC-I4), so
// `expect_after.timeout_ms` runs out after exactly `timeout_ms / interval_ms` polls. Every expected
// value comes from the model row L4b-T.

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_contract::{
    InstanceFactContext, InstanceFactSnapshot, ResourceDeclarationReason,
};
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::{
    MAX_IDENTITY_TEMPLATES, NnProviderRequest, NnProviderResult, OcrProviderRequest,
    OcrProviderResult, OcrProviderTextBlock, VisionProviderError, VisionProviderErrorCode,
};
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const WIDTH: u32 = 400;
const HEIGHT: u32 = 600;
const TASK_TIMEOUT_MS: u64 = 300_000;
const SCAN_MAX_MS: u64 = 240_000;
const INTERVAL_MS: u64 = 200;
const POLICY_ENTRY: &str = "resources/operations/task/policies/select.json";
const DETAIL: &str = "neutral/detail";
// The identity domain and the name each member's code reads as.
const MEMBERS: [&str; 4] = ["member.a", "member.b", "member.c", "member.d"];
const NAMES: [&str; 4] = ["alpha", "bravo", "charlie", "delta"];
// Members by index, and the seed of the second image of `member.b`.
const A: usize = 0;
const B: usize = 1;
const C: usize = 2;
const D: usize = 3;
const B_SECOND: u32 = 7;
const CARD_SEED: u32 = 9;
const SIDE: u32 = 12;
const BACKGROUND: [u8; 3] = [30, 31, 32];
const GREEN: [u8; 3] = [0, 255, 0];
const HOME_COLOUR: [u8; 3] = [255, 255, 0];
const LIST_COLOUR: [u8; 3] = [255, 0, 0];
const DETAIL_COLOUR: [u8; 3] = [0, 0, 255];

// Deterministic pixel noise of one image: a shifted or other image correlates with it near zero.
fn noise(seed: u32, x: u32, y: u32) -> [u8; 3] {
    let mut state = ((u64::from(seed) << 32) | (u64::from(y) << 16) | u64::from(x))
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ 0x2545_f491_4f6c_dd1d;
    let mut next = || {
        state ^= state >> 33;
        state = state.wrapping_mul(0xff51_afd7_ed55_8ccd);
        state ^= state >> 33;
        (state & 0xff) as u8
    };
    [next(), next(), next()]
}

fn seed(member: usize) -> u32 {
    u32::try_from(member).expect("member index")
}

fn png(seed: u32, width: u32, height: u32) -> Vec<u8> {
    Frame::from_pixels(
        width,
        height,
        (0..height)
            .flat_map(|y| (0..width).flat_map(move |x| noise(seed, x, y)))
            .collect(),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("image frame")
    .png_for_artifact()
    .expect("image PNG")
}

// What the device shows.
#[derive(Debug, Clone)]
enum Screen {
    Home,
    // The list's three slots: the member each one shows (its image and its name code) and
    // whether it is open.
    List([(usize, bool); 3]),
    // The destination page with these images (seeds) in its identity region, left to right.
    Detail(Vec<u32>),
}

fn put(pixels: &mut [[u8; 3]], x: u32, y: u32, colour: [u8; 3]) {
    pixels[usize::try_from(y * WIDTH + x).expect("pixel index")] = colour;
}

fn stamp(pixels: &mut [[u8; 3]], seed: u32, left: u32, top: u32) {
    for y in 0..SIDE {
        for x in 0..SIDE {
            put(pixels, left + x, top + y, noise(seed, x, y));
        }
    }
}

fn frame(screen: &Screen) -> ScriptedFrame {
    let mut pixels = vec![BACKGROUND; usize::try_from(WIDTH * HEIGHT).expect("frame size")];
    match screen {
        Screen::Home => put(&mut pixels, 0, 0, HOME_COLOUR),
        Screen::List(slots) => {
            put(&mut pixels, 0, 0, LIST_COLOUR);
            put(&mut pixels, 3, 0, GREEN);
            for (left, (member, open)) in (0..).map(|slot: u32| 120 * slot).zip(slots) {
                if *open {
                    put(&mut pixels, 22 + left, 102, GREEN);
                }
                let code = u8::try_from(member + 1).expect("name code");
                put(&mut pixels, 24 + left, 102, [code, 0, 0]);
                stamp(&mut pixels, seed(*member), 44 + left, 114);
            }
        }
        Screen::Detail(images) => {
            put(&mut pixels, 0, 0, DETAIL_COLOUR);
            for (left, image) in (0..).map(|spot: u32| 104 + 20 * spot).zip(images) {
                stamp(&mut pixels, *image, left, 304);
            }
        }
    }
    ScriptedFrame::rgb(WIDTH, HEIGHT, &pixels)
}

// The list with `member.a`, `member.b` and `member.c`, only `member.b`'s slot open.
fn list_choosing_b() -> Screen {
    Screen::List([(A, false), (B, true), (C, false)])
}

fn detail(members: &[usize]) -> Screen {
    Screen::Detail(members.iter().copied().map(seed).collect())
}

// Reads the name of the code in the pixel its region starts on at confidence 0.99; any other
// pixel reads no text.
#[derive(Debug)]
struct ShownNames;

impl VisionProvider for ShownNames {
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
        let x = u32::try_from(request.region.x).expect("a region on the frame");
        let y = u32::try_from(request.region.y).expect("a region on the frame");
        let index = usize::try_from((y * request.frame.width + x) * 3).expect("pixel index");
        let text = usize::from(request.frame.rgb8_pixels[index])
            .checked_sub(1)
            .and_then(|code| NAMES.get(code).copied())
            .unwrap_or_default();
        let (blocks, confidence) = if text.is_empty() {
            (Vec::new(), None)
        } else {
            (
                vec![OcrProviderTextBlock {
                    text: text.to_owned(),
                    rect: request.region,
                    confidence: Some(0.99),
                }],
                Some(0.99),
            )
        };
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: text.to_owned(),
            blocks,
            confidence,
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

// The Runtime as the kernel sees it: the scripted runtime (its frames, inputs, traces and clock)
// with an empty fact snapshot for select steps. It keeps how many frames were captured when each
// input was made.
struct Device {
    script: ScriptedRuntime,
    captured_at_input: Vec<usize>,
}

impl Device {
    // A device that shows `stages[n]` after `n` inputs, the last screen of a stage staying.
    fn new(stages: &[Vec<Screen>]) -> Self {
        Self {
            script: ScriptedRuntime::new(
                stages
                    .iter()
                    .map(|screens| screens.iter().map(frame).collect())
                    .collect(),
            ),
            captured_at_input: Vec::new(),
        }
    }

    // The frames captured after the first input: the destination frames of a one-tap run.
    fn captured_after_input(&self) -> usize {
        let before = self
            .captured_at_input
            .first()
            .copied()
            .expect("the run made an input");
        self.script.captures() - before
    }

    fn inputs(&self) -> &[InputAction] {
        self.script.inputs()
    }

    // The step, selection, effect and final traces, in order.
    fn kinds(&self) -> Vec<&'static str> {
        self.script
            .traces()
            .iter()
            .filter_map(|trace| match trace {
                ContainedTaskTrace::StepStarted { .. } => Some("step_started"),
                ContainedTaskTrace::SelectionEvaluated { .. } => Some("selection_evaluated"),
                ContainedTaskTrace::EffectIntent { .. } => Some("effect_intent"),
                ContainedTaskTrace::EffectCompleted { .. } => Some("effect_completed"),
                ContainedTaskTrace::StepFinished { .. } => Some("step_finished"),
                ContainedTaskTrace::Finalizing { .. } => Some("finalizing"),
                _ => None,
            })
            .collect()
    }

    fn finished(&self, operation: &str, page: &str) -> bool {
        self.script
            .traces()
            .contains(&ContainedTaskTrace::StepFinished {
                step_index: 0,
                operation_label: operation.to_owned(),
                page_label: page.to_owned(),
                phase: None,
            })
    }
}

impl ContainedTaskRuntime for Device {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        self.script.capture()
    }

    fn input(
        &mut self,
        action: InputAction,
        frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.captured_at_input.push(self.script.captures());
        self.script.input(action, frame)
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        self.script.record(trace)
    }

    fn selection_state(
        &mut self,
        request: SelectionStateRequest,
    ) -> Result<SelectionState, Self::Error> {
        Ok(SelectionState::Snapshot {
            snapshot: InstanceFactSnapshot {
                snapshot_id: "snapshot:test:empty".to_owned(),
                ledger_position: 1,
                context: InstanceFactContext {
                    instance_id: "neutral.instance".to_owned(),
                    server_id: request.server,
                    game_id: request.game,
                },
                records: Vec::new(),
            },
            now_unix_ms: 1_000_000,
        })
    }

    fn monotonic_now(&mut self) -> Instant {
        self.script.monotonic_now()
    }

    fn pause(&mut self, duration: Duration) {
        self.script.pause(duration);
    }
}

fn rect(x: u32, y: u32, width: u32, height: u32) -> Value {
    json!({"x": x, "y": y, "width": width, "height": height})
}

fn pixel(x: u32, y: u32) -> Value {
    rect(x, y, 1, 1)
}

// The destination's identity region.
fn title_region() -> Value {
    rect(100, 300, 60, 30)
}

// Where slot `slot`'s image is searched, inside its slot.
fn icon_region(slot: u32) -> Value {
    rect(40 + 120 * slot, 110, 24, 24)
}

fn image_path(member: usize) -> String {
    format!("assets/member-{member}.png")
}

fn title(member: usize) -> String {
    format!("title/member_{member}")
}

fn color(id: &str, region: Value, expected: [u8; 3]) -> Value {
    json!({"type": "color", "id": id, "region": region, "expected": expected})
}

fn ocr(id: &str, region: Value) -> Value {
    json!({"type": "ocr", "id": id, "region": region, "languages": ["en"], "timeout_ms": 1000,
           "match_mode": "exact", "expected": ["0"], "case_sensitive": false,
           "minimum_confidence": 0.0, "model_ref": "PP-OCRv6_medium",
           "model_sha256": "a".repeat(64)})
}

fn template(id: &str, path: &str, region: Value, threshold: f64) -> Value {
    json!({"type": "template", "id": id, "template_path": path, "region": region,
           "threshold": threshold})
}

// Every target of the pack: the pages, the guard and the scan checks; the card list's anchor,
// name and open mark (the scan select of L4bT-T6); each slot's open mark, name and member images;
// and the destination's pool: one title of each member, the second image of `member.b` and a
// title of threshold 0, which always passes.
fn targets() -> Vec<Value> {
    let mut targets = vec![
        color("page/home", pixel(0, 0), HOME_COLOUR),
        color("page/list", pixel(0, 0), LIST_COLOUR),
        color("page/detail", pixel(0, 0), DETAIL_COLOUR),
        color("check/list_start", pixel(1, 0), GREEN),
        color("check/list_end", pixel(2, 0), [0, 0, 255]),
        color("guard/list", pixel(3, 0), GREEN),
        template("ui/card", "assets/card.png", json!("full_frame"), 0.9),
        ocr("card/name", rect(0, 0, 100, 20)),
        color("card/open", rect(0, 0, 20, 20), GREEN),
        ocr("detail/name", rect(100, 100, 200, 40)),
    ];
    for slot in 0..3 {
        let left = 120 * slot;
        targets.push(color(
            &format!("slot/open_{slot}"),
            pixel(22 + left, 102),
            GREEN,
        ));
        targets.push(ocr(&format!("slot/name_{slot}"), pixel(24 + left, 102)));
        for member in 0..MEMBERS.len() {
            targets.push(template(
                &format!("slot/icon_{slot}_{member}"),
                &image_path(member),
                icon_region(slot),
                0.9,
            ));
        }
    }
    for member in 0..MEMBERS.len() {
        targets.push(template(
            &title(member),
            &image_path(member),
            title_region(),
            0.9,
        ));
    }
    targets.push(template(
        "title/member_1_second",
        "assets/member-1-second.png",
        title_region(),
        0.9,
    ));
    targets.push(template(
        "title/always",
        &image_path(D),
        title_region(),
        0.0,
    ));
    targets
}

fn member_entries(aliases: bool) -> Vec<Value> {
    MEMBERS
        .iter()
        .zip(NAMES)
        .map(|(id, name)| {
            if aliases {
                json!({"id": id, "aliases": [name]})
            } else {
                json!({"id": id})
            }
        })
        .collect()
}

fn text_identity() -> Value {
    json!({"entries": member_entries(true),
           "recognition": {"kind": "ocr_aliases", "max_distance": 1, "minimum_margin": 1,
                           "minimum_confidence_milli": 800}})
}

fn open_feature() -> Value {
    json!({"name": "open", "value": "passed"})
}

fn slot(index: u32, targets: Value) -> Value {
    let left = 120 * index;
    json!({"rect": rect(20 + left, 100, 100, 100), "click": rect(30 + left, 160, 40, 30),
           "targets": targets})
}

// The list's slots, each member told by its image (an icon identity of four IDs).
fn icon_layout() -> Value {
    let slots = (0..3)
        .map(|index| {
            let mut slot = slot(index, json!({"open": format!("slot/open_{index}")}));
            let pool = (0..MEMBERS.len())
                .map(|member| {
                    json!({"id": MEMBERS[member],
                           "target_id": format!("slot/icon_{index}_{member}")})
                })
                .collect::<Vec<_>>();
            slot["identity_templates"] = json!({"member": pool});
            slot
        })
        .collect::<Vec<_>>();
    json!({
        "id": "layout/icons", "page_id": "neutral/list", "kind": "fixed_slots",
        "features": [
            {"name": "member", "value": "identity",
             "identity": {"entries": member_entries(false),
                          "recognition": {"kind": "icon_templates", "minimum_score_milli": 900,
                                          "minimum_margin_milli": 50}}},
            open_feature()
        ],
        "slots": slots
    })
}

// The list's slots, each member told by its name (a text identity).
fn name_layout() -> Value {
    let slots = (0..3)
        .map(|index| {
            slot(
                index,
                json!({"member": format!("slot/name_{index}"),
                       "open": format!("slot/open_{index}")}),
            )
        })
        .collect::<Vec<_>>();
    json!({
        "id": "layout/names", "page_id": "neutral/list", "kind": "fixed_slots",
        "features": [{"name": "member", "value": "identity", "identity": text_identity()},
                     open_feature()],
        "slots": slots
    })
}

// The list's slots told only by their open mark: a layout without an identity feature.
fn open_layout() -> Value {
    let slots = (0..3)
        .map(|index| slot(index, json!({"open": format!("slot/open_{index}")})))
        .collect::<Vec<_>>();
    json!({
        "id": "layout/slots", "page_id": "neutral/list", "kind": "fixed_slots",
        "features": [open_feature()],
        "slots": slots
    })
}

// A repeated-anchor card list read by a scan (the SEL-L4b admission gate's list), its member
// told by its name.
fn card_layout() -> Value {
    json!({
        "id": "layout/cards", "page_id": "neutral/list", "kind": "repeated_anchor",
        "anchor": "ui/card", "max_instances": 64, "order": "top_to_bottom",
        "instance_rect": {"x": 0, "y": 0, "width": 100, "height": 80},
        "click": {"x": 10, "y": 10, "width": 80, "height": 40},
        "readable_band": {"x": 0, "y": 60, "width": 400, "height": 480},
        "unknown_identity": "reject",
        "features": [
            {"name": "member", "value": "identity", "target": "card/name",
             "offset": {"x": 0, "y": 50}, "identity": text_identity()},
            {"name": "open", "value": "passed", "target": "card/open",
             "offset": {"x": 80, "y": 0}}
        ]
    })
}

fn scan_declaration() -> Value {
    json!({
        "identity_feature": "member",
        "axis": "vertical",
        "advance": {"kind": "drag",
                    "from": {"x": 150, "y": 450, "width": 100, "height": 20},
                    "to": {"x": 150, "y": 150, "width": 100, "height": 20},
                    "duration_ms": 500, "min_content_step_px": 180, "max_content_step_px": 400},
        "reposition": {"kind": "drag",
                       "from": {"x": 150, "y": 500, "width": 100, "height": 20},
                       "to": {"x": 150, "y": 80, "width": 100, "height": 20},
                       "duration_ms": 400, "max_content_step_px": 480},
        "settle": {"interval_ms": 200, "timeout_ms": 1000},
        "track": {"area": {"x": 0, "y": 40, "width": 400, "height": 520},
                  "edge_margin_px": 12,
                  "signature": {"rect": {"x": 0, "y": 0, "width": 20, "height": 20},
                                "columns": 1, "rows": 1},
                  "signature_max_distance_milli": 500,
                  "position_tolerance_px": 4,
                  "min_overlap_instances": 2,
                  "spacing": {"max_axis_gap_px": 120, "columns": 3, "max_trailing_gap_px": 200,
                              "max_cross_lead_px": 40, "max_cross_gap_px": 140}},
        "start": {"check": "check/list_start", "rewind": true},
        "end": {"check": "check/list_end"},
        "bounds": {"max_viewports": 32, "max_reposition_viewports": 8, "max_ms": SCAN_MAX_MS}
    })
}

// The base policy over `layout`: every open slot is a member, of equal score; mode
// `none_allowed` with one candidate; the lowest candidate ID last.
fn policy(layout: &str, identity: bool) -> Value {
    let mut fields = vec![json!({"name": "open", "value_type": {"type": "boolean"}})];
    if identity {
        fields.insert(
            0,
            json!({"name": "member",
                   "value_type": {"type": "enum_string", "allowed": MEMBERS}}),
        );
    }
    json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "members",
        "applies_to": {
            "candidate_layout_id": layout,
            "outcome_keys": {"selected": "card_selected", "empty": "card_empty",
                             "insufficient": "card_insufficient", "ambiguous": "card_ambiguous",
                             "unknown": "card_unknown"}
        },
        "fields": fields,
        "facts": [],
        "gates": [{"gate_id": "open",
                   "predicate": {"kind": "boolean_equals",
                                 "value": {"source": "field", "field": "open"},
                                 "expected": true},
                   "on_unknown": {"kind": "drop_candidate"}}],
        "scoring": [],
        "selection": {"mode": "none_allowed", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    })
}

// The pool of one title per member, in the identity region.
fn pool() -> Value {
    json!(
        (0..MEMBERS.len())
            .map(|member| json!({"id": MEMBERS[member], "target_id": title(member)}))
            .collect::<Vec<_>>()
    )
}

// The pool with the second image of `member.b` as a fifth entry.
fn pool_with_second_b() -> Value {
    let mut pool = pool();
    pool.as_array_mut()
        .expect("pool")
        .push(json!({"id": MEMBERS[B], "target_id": "title/member_1_second"}));
    pool
}

// The destination page within `timeout_ms`, polled every `INTERVAL_MS`, whose identity region
// must show the business ID `choice_of` chose, read through `pool`.
fn expecting(timeout_ms: u64, choice_of: &str, pool: Value) -> Value {
    json!({"page_id": "detail", "timeout_ms": timeout_ms, "interval_ms": INTERVAL_MS,
           "expect_identity": {"choice_of": choice_of, "templates": pool}})
}

// The select step `pick`: guarded, from the list over `layout`, a tap into the detail page.
fn pick(layout: &str, expect_after: Value) -> Value {
    json!({
        "id": "pick", "from": "list", "to": "detail",
        "expect_after": expect_after,
        "guard": {"page_id": "list", "target_id": "guard/list", "expected_rect": pixel(3, 0),
                  "color_probe": "guard/list"},
        "select": {"layout_id": layout, "policy": {"path": "policies/select.json"}}
    })
}

// A tap `open` from the home page into the detail page.
fn open_from_home(expect_after: Value) -> Value {
    json!({"id": "open", "from": "home", "to": "detail",
           "click": {"kind": "point", "x": 200, "y": 500},
           "unguarded_trusted_coordinate": true, "retryable": false,
           "expect_after": expect_after})
}

// A page-graph task (schema 0.6) to the detail page over `layout` through `operations`.
fn task(layout: Value, operations: Vec<Value>) -> Value {
    json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "target_page": "detail",
        "candidate_layouts": [layout],
        "operations": operations
    })
}

// The task whose one step `pick` selects a slot by its image and expects `expect_after`.
fn icon_task(expect_after: Value) -> Value {
    task(icon_layout(), vec![pick("layout/icons", expect_after)])
}

fn icon_policy() -> Value {
    policy("layout/icons", true)
}

// The scan select over the card list (a text identity), its destination confirmed by the pool.
fn scan_task() -> Value {
    let mut step = pick("layout/cards", expecting(5_000, "pick", pool()));
    step["select"]["scan"] = scan_declaration();
    let mut task = task(card_layout(), vec![step]);
    task["scheduling_outcome"] = json!({"mappings": [
        {"outcome_key": "card_selected", "effect": "no_designated_effect",
         "terminal_pages": ["detail"]}
    ]});
    task
}

// The recognition pack of `task`: every target and the task's candidate layouts.
fn pack(task: &Value) -> Value {
    json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "defaults": {"match_metric": "ccoeff_normed", "color_max_distance": 0.0},
        "targets": targets(),
        "candidate_layouts": task["candidate_layouts"].clone()
    })
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("JSON bytes")
}

// The package of `task` with `policy` as its select step's document and `pack` as its
// recognition pack; the images are entries of the package.
fn package(task: &Value, policy: &Value, pack: &Value) -> Vec<u8> {
    let control = json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.member.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": HEIGHT},
        "entry_task_id": "task",
        "capture_interval_ms": 100,
        "step_timeout_ms": 10_000,
        "timeout_ms": TASK_TIMEOUT_MS
    });
    let pages = ["home", "list", "detail"].map(|page| {
        json!({"id": format!("neutral/{page}"), "required": [format!("page/{page}")],
               "optional": [], "forbidden": []})
    });
    let mut entries = vec![
        ("control.json".to_owned(), bytes(&control)),
        (
            "resources/manifest.json".to_owned(),
            bytes(&json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        (
            "resources/operations/task/task.json".to_owned(),
            bytes(task),
        ),
        (POLICY_ENTRY.to_owned(), bytes(policy)),
        (
            "resources/assets/card.png".to_owned(),
            png(CARD_SEED, 16, 12),
        ),
        (
            "resources/assets/member-1-second.png".to_owned(),
            png(B_SECOND, SIDE, SIDE),
        ),
        (
            "resources/recognition/neutral.test.pack.json".to_owned(),
            bytes(pack),
        ),
        (
            "resources/recognition/neutral.test.pages.json".to_owned(),
            bytes(&json!({"schema_version": "0.3", "pages": pages})),
        ),
    ];
    for member in 0..MEMBERS.len() {
        entries.push((
            format!("resources/{}", image_path(member)),
            png(seed(member), SIDE, SIDE),
        ));
    }
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, content) in &entries {
        zip.start_file(path.as_str(), options).expect("zip entry");
        zip.write_all(content).expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

fn admit_pack(
    task: &Value,
    policy: &Value,
    pack: &Value,
) -> Result<PreparedContainedTask, ContainedTaskError> {
    PreparedContainedTask::load_with_vision_provider(
        "neutral.instance",
        &package(task, policy, pack),
        Arc::new(ShownNames),
    )
}

fn admitted(case: &str, task: &Value, policy: &Value) -> PreparedContainedTask {
    admit_pack(task, policy, &pack(task)).unwrap_or_else(|error| panic!("{case}: refused: {error}"))
}

// The refusal of the package, which must be `contained_task_select_invalid` whose detail names
// `token`.
fn refused(case: &str, task: &Value, policy: &Value, token: &str) {
    let error = match admit_pack(task, policy, &pack(task)) {
        Ok(_) => panic!("{case}: admitted"),
        Err(error) => error,
    };
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::ContainedTaskSelectInvalid.as_str(),
        "{case}: {error}"
    );
    let detail = error.detail().unwrap_or_default();
    assert!(
        detail.contains(token),
        "{case}: the detail {detail:?} does not name {token:?}"
    );
}

fn succeeded(
    result: Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>,
) -> ContainedTaskOutcome {
    result.unwrap_or_else(|error| panic!("expected the run to end normally, got {error:?}"))
}

fn failed(
    result: Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>,
) -> ContainedTaskError {
    match result {
        Err(ContainedTaskRunError::Task(error)) => error,
        Err(other) => panic!("expected a task failure, got {other:?}"),
        Ok(outcome) => panic!("expected a task failure, the run ended {outcome:?}"),
    }
}

// The run's normal end on the detail page after its one step.
fn completed_on_detail() -> ContainedTaskOutcome {
    ContainedTaskOutcome {
        outcome: TaskOutcome::Success,
        final_page: Some(DETAIL.to_owned()),
        executed_steps: 1,
        selected_scheduling_outcome: None,
        designated_effect_caps: Vec::new(),
    }
}

// `selection_identity_mismatch` at `select_expect_identity` with `state`, the expected and the
// observed business ID.
fn assert_mismatch(
    error: &ContainedTaskError,
    state: &str,
    expected: Option<&str>,
    observed: Option<&str>,
) {
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::SelectionIdentityMismatch.as_str(),
        "{error}"
    );
    let outcome = error.outcome();
    assert_eq!(
        outcome.value("stage"),
        Some(&json!("select_expect_identity"))
    );
    assert_eq!(outcome.value("identity_state"), Some(&json!(state)));
    assert_eq!(
        outcome.value("expected_identity"),
        expected.map(|id| json!(id)).as_ref()
    );
    assert_eq!(
        outcome.value("observed_identity"),
        observed.map(|id| json!(id)).as_ref()
    );
}

const AFTER_ONE_TAP: [&str; 5] = [
    "step_started",
    "selection_evaluated",
    "effect_intent",
    "effect_completed",
    "step_finished",
];

// The one tap of `pick` lies in the click box of `member.b`'s slot (x 150, y 160, 40 x 30).
fn assert_tapped_b(device: &Device) {
    assert!(
        matches!(
            device.inputs(),
            [InputAction::Tap {
                x: 150..=189,
                y: 160..=189
            }]
        ),
        "{:?}",
        device.inputs()
    );
}

// L4bT-T1 (L4b-T.2, L4b-T.4 matched): a single-frame select over three slots told by their
// images (an icon identity of four IDs) chooses `member.b`, the one open slot, and taps it. Its
// destination shows `member.b`'s title in the identity region, so `member.b`'s pool entry passes
// on the first destination frame: the step completes there and the run ends on the detail page,
// with no further capture and no further input.
#[test]
fn l4bt_an_icon_choice_is_matched_by_its_title_on_the_first_destination_frame() {
    let task = admitted(
        "an icon choice",
        &icon_task(expecting(5_000, "pick", pool())),
        &icon_policy(),
    );
    let mut device = Device::new(&[vec![list_choosing_b()], vec![detail(&[B])]]);

    let outcome = succeeded(task.run(&mut device));

    assert_eq!(outcome, completed_on_detail());
    assert_tapped_b(&device);
    assert_eq!(device.captured_after_input(), 1);
    let mut kinds = AFTER_ONE_TAP.to_vec();
    kinds.push("finalizing");
    assert_eq!(device.kinds(), kinds);
    assert!(device.finished("pick", DETAIL));
}

// L4bT-T2 (L4b-T.4 absent, then matched): the first destination frame shows no title, so no
// entry passes and the step polls again; the next frame shows `member.b`'s title and the step
// completes after exactly one more capture, with no input after the tap.
#[test]
fn l4bt_a_late_title_is_matched_on_the_next_poll() {
    let task = admitted(
        "an icon choice",
        &icon_task(expecting(5_000, "pick", pool())),
        &icon_policy(),
    );
    let mut device = Device::new(&[vec![list_choosing_b()], vec![detail(&[]), detail(&[B])]]);

    let outcome = succeeded(task.run(&mut device));

    assert_eq!(outcome, completed_on_detail());
    assert_tapped_b(&device);
    assert_eq!(device.captured_after_input(), 2);
    assert!(device.finished("pick", DETAIL));
}

// L4bT-T3 (L4b-T.4 different): the destination shows `member.c`'s title, another Known business
// ID. The step fails at once on that first destination frame with `selection_identity_mismatch`
// (`different`, `member.b` expected, `member.c` observed), after its `StepFinished`, with no
// further capture and no further input.
#[test]
fn l4bt_another_member_s_title_fails_at_once_as_different() {
    let task = admitted(
        "an icon choice",
        &icon_task(expecting(5_000, "pick", pool())),
        &icon_policy(),
    );
    let mut device = Device::new(&[vec![list_choosing_b()], vec![detail(&[C])]]);

    let error = failed(task.run(&mut device));

    assert_mismatch(&error, "different", Some(MEMBERS[B]), Some(MEMBERS[C]));
    assert_tapped_b(&device);
    assert_eq!(device.captured_after_input(), 1);
    assert_eq!(device.kinds(), AFTER_ONE_TAP);
    assert!(device.finished("pick", DETAIL));
}

// L4bT-T4 (L4b-T.4 unknown): every destination frame shows the titles of `member.b` and
// `member.c`, two business IDs, which is no reading. The step polls until `expect_after.timeout_ms`
// (1 000 ms at 200 ms: the first frame and five polls) and then fails with `unknown`, `member.b`
// expected and nothing observed, after its `StepFinished`.
#[test]
fn l4bt_two_members_passing_until_the_timeout_fail_as_unknown() {
    let task = admitted(
        "an icon choice",
        &icon_task(expecting(1_000, "pick", pool())),
        &icon_policy(),
    );
    let mut device = Device::new(&[vec![list_choosing_b()], vec![detail(&[B, C])]]);

    let error = failed(task.run(&mut device));

    assert_mismatch(&error, "unknown", Some(MEMBERS[B]), None);
    assert_tapped_b(&device);
    assert_eq!(device.captured_after_input(), 6);
    assert_eq!(device.kinds(), AFTER_ONE_TAP);
    assert!(device.finished("pick", DETAIL));
}

// L4bT-T5 (L4b-T.4 absent; Q3): no destination frame shows a title, so no entry passes. The step
// polls until `expect_after.timeout_ms` (the first frame and five polls) and then fails with
// `absent`, reported apart from `unknown`, `member.b` expected and nothing observed.
#[test]
fn l4bt_no_member_passing_until_the_timeout_fails_as_absent() {
    let task = admitted(
        "an icon choice",
        &icon_task(expecting(1_000, "pick", pool())),
        &icon_policy(),
    );
    let mut device = Device::new(&[vec![list_choosing_b()], vec![detail(&[])]]);

    let error = failed(task.run(&mut device));

    assert_mismatch(&error, "absent", Some(MEMBERS[B]), None);
    assert_tapped_b(&device);
    assert_eq!(device.captured_after_input(), 6);
    assert_eq!(device.kinds(), AFTER_ONE_TAP);
    assert!(device.finished("pick", DETAIL));
}

// L4bT-T6 (L4b-T.2, Q4): the recognition kind of the choice does not matter. A single-frame
// select over slots told by their names (`ocr_aliases`) chooses `member.b`, and its destination's
// template pool reads `member.b`: matched. With the second image of `member.b` in the pool and
// both images shown, two entries of one business ID pass, which still reads `member.b`: matched.
// A select over a scan (`repeated_anchor`, a text identity) confirmed by the pool is admitted, and
// its `expect_identity` passes on a destination frame showing the title of the business ID the
// step chose.
#[test]
fn l4bt_a_text_choice_single_frame_or_scanned_is_matched_by_the_pool() {
    let name_policy = policy("layout/names", true);
    for (case, pool, shown) in [
        ("one title", pool(), Screen::Detail(vec![seed(B)])),
        (
            "two images of the chosen member",
            pool_with_second_b(),
            Screen::Detail(vec![seed(B), B_SECOND]),
        ),
    ] {
        let task = admitted(
            case,
            &task(
                name_layout(),
                vec![pick("layout/names", expecting(5_000, "pick", pool))],
            ),
            &name_policy,
        );
        let mut device = Device::new(&[vec![list_choosing_b()], vec![shown]]);

        let outcome = task
            .run(&mut device)
            .unwrap_or_else(|error| panic!("{case}: {error:?}"));

        assert_eq!(outcome, completed_on_detail(), "{case}");
        assert_tapped_b(&device);
        assert_eq!(device.captured_after_input(), 1, "{case}");
    }

    let scanned = admitted("a scan select", &scan_task(), &policy("layout/cards", true));
    let operation = &scanned.program.operations[0];
    assert_eq!(operation.scan_max_ms(), Some(SCAN_MAX_MS));
    let mut device = Device::new(&[vec![detail(&[B])]]);
    let started = device.monotonic_now();
    let timing = ContainedTaskTimingContext::new(
        started,
        started + Duration::from_millis(TASK_TIMEOUT_MS),
        actingcommand_contract::TaskTimingBudgetOrigin::Task,
    );
    let mut observation = scanned
        .capture_frame(&mut device, None, None, timing, None, None)
        .expect("the destination frame");
    let chosen = BTreeMap::from([("pick".to_owned(), MEMBERS[B].to_owned())]);
    scanned
        .expect_identity(
            &mut device,
            operation,
            (0, "pick"),
            &mut observation,
            &chosen,
            timing,
        )
        .expect("the scan select's choice is read back through the pool");
    assert_eq!(device.script.captures(), 1);
}

// L4bT-T7 (L4b-T.4 no choice): the `choice_of` step made no choice in this run. The run starts on
// the home page, so `pick` of the list never runs on this route; the tap `open` from the home page
// into the detail page expects `pick`'s choice through the pool. Although the destination shows a
// title, the step fails `selection_identity_mismatch` with `no_choice` and no expected or observed
// ID, after its `StepFinished`, with its one input, as on the text path.
#[test]
fn l4bt_an_expectation_of_a_step_that_did_not_choose_fails_no_choice() {
    let task = admitted(
        "an expectation of a step that did not choose",
        &task(
            icon_layout(),
            vec![
                pick(
                    "layout/icons",
                    json!({"page_id": "detail", "timeout_ms": 5_000, "interval_ms": INTERVAL_MS}),
                ),
                open_from_home(expecting(5_000, "pick", pool())),
            ],
        ),
        &icon_policy(),
    );
    let mut device = Device::new(&[vec![Screen::Home], vec![detail(&[B])]]);

    let error = failed(task.run(&mut device));

    assert_mismatch(&error, "no_choice", None, None);
    assert_eq!(device.inputs().len(), 1, "{:?}", device.inputs());
    assert_eq!(
        device.kinds(),
        [
            "step_started",
            "effect_intent",
            "effect_completed",
            "step_finished"
        ]
    );
    assert!(device.finished("open", DETAIL));
}

// L4bT-T8 (L4b-T.3): admission. The base pool on an icon choice and on a text choice is admitted;
// each refusal changes the admitted base in the one declaration under test and is
// `contained_task_select_invalid` with its detail token. The text path with an icon choice is
// still refused `choice_of=identity_not_text`.
#[test]
fn l4bt_admission_refuses_each_rule_of_the_template_path_with_its_token() {
    let base = icon_task(expecting(5_000, "pick", pool()));
    admitted("the pool on an icon choice", &base, &icon_policy());
    admitted(
        "the pool on a text choice",
        &task(
            name_layout(),
            vec![pick("layout/names", expecting(5_000, "pick", pool()))],
        ),
        &policy("layout/names", true),
    );
    let with_identity = |identity: Value| {
        let mut task = base.clone();
        task["operations"][0]["expect_after"]["expect_identity"] = identity;
        task
    };
    let entries = |extra: Value| {
        let mut pool = pool();
        pool.as_array_mut().expect("pool").push(extra);
        with_identity(json!({"choice_of": "pick", "templates": pool}))
    };
    let mut retryable = base.clone();
    retryable["operations"][0]["retryable"] = json!(true);
    retryable["operations"][0]["max_attempts"] = json!(2);
    retryable["operations"][0]["retry_interval_ms"] = json!(500);
    let over_bound = (0..MAX_IDENTITY_TEMPLATES + 1)
        .map(|index| {
            let member = index % MEMBERS.len();
            json!({"id": MEMBERS[member], "target_id": title(member)})
        })
        .collect::<Vec<_>>();
    let plain = || json!({"page_id": "detail", "timeout_ms": 5_000, "interval_ms": INTERVAL_MS});
    // The tap `open` declared first, expecting the choice of `pick`, declared after it.
    let open_first = task(
        icon_layout(),
        vec![
            open_from_home(expecting(5_000, "pick", pool())),
            pick("layout/icons", plain()),
        ],
    );
    // `pick` expecting the choice of the tap `open`.
    let open_chosen = task(
        icon_layout(),
        vec![
            open_from_home(plain()),
            pick("layout/icons", expecting(5_000, "open", pool())),
        ],
    );
    let icon = icon_policy();
    for (case, task, policy, token) in [
        (
            "a choice_of no operation declares",
            with_identity(json!({"choice_of": "nobody", "templates": pool()})),
            icon.clone(),
            "operation=pick expect_identity choice_of=undeclared",
        ),
        (
            "a choice_of that is no select step",
            open_chosen,
            icon.clone(),
            "operation=pick expect_identity choice_of=not_a_select_step",
        ),
        (
            "a choice_of after the declaring step",
            open_first,
            icon.clone(),
            "operation=open expect_identity choice_of=after_the_step",
        ),
        (
            "a choice_of step without an identity",
            task(
                open_layout(),
                vec![pick("layout/slots", expecting(5_000, "pick", pool()))],
            ),
            policy("layout/slots", false),
            "operation=pick expect_identity choice_of=no_identity",
        ),
        (
            "both target_id and templates",
            with_identity(
                json!({"choice_of": "pick", "target_id": "detail/name", "templates": pool()}),
            ),
            icon.clone(),
            "operation=pick expect_identity expect_identity=target_and_templates",
        ),
        (
            "neither target_id nor templates",
            with_identity(json!({"choice_of": "pick"})),
            icon.clone(),
            "operation=pick expect_identity expect_identity=neither",
        ),
        (
            "a retryable step",
            retryable,
            icon.clone(),
            "operation=pick expect_identity operation=retryable",
        ),
        (
            "an empty pool",
            with_identity(json!({"choice_of": "pick", "templates": []})),
            icon.clone(),
            "operation=pick expect_identity templates=count",
        ),
        (
            "a pool over its bound",
            with_identity(json!({"choice_of": "pick", "templates": over_bound})),
            icon.clone(),
            "operation=pick expect_identity templates=count",
        ),
        (
            "an OCR target",
            entries(json!({"id": MEMBERS[A], "target_id": "detail/name"})),
            icon.clone(),
            "operation=pick expect_identity template=not_template",
        ),
        (
            "a colour target",
            entries(json!({"id": MEMBERS[A], "target_id": "page/detail"})),
            icon.clone(),
            "operation=pick expect_identity template=not_template",
        ),
        (
            "an undeclared target",
            entries(json!({"id": MEMBERS[A], "target_id": "title/missing"})),
            icon.clone(),
            "operation=pick expect_identity template=not_template",
        ),
        (
            "a target named twice",
            entries(json!({"id": MEMBERS[C], "target_id": title(B)})),
            icon.clone(),
            "operation=pick expect_identity template=duplicate",
        ),
        (
            "an ID outside the domain",
            entries(json!({"id": "member.z", "target_id": "title/member_1_second"})),
            icon.clone(),
            "operation=pick expect_identity template=out_of_domain",
        ),
        (
            "a target of threshold 0",
            entries(json!({"id": MEMBERS[D], "target_id": "title/always"})),
            icon.clone(),
            "operation=pick expect_identity template=threshold_zero",
        ),
        (
            "a full-frame target",
            entries(json!({"id": MEMBERS[A], "target_id": "ui/card"})),
            icon.clone(),
            "operation=pick expect_identity template=region_not_rect",
        ),
        (
            "a target in another rect",
            entries(json!({"id": MEMBERS[B], "target_id": "slot/icon_0_1"})),
            icon.clone(),
            "operation=pick expect_identity template=regions_differ",
        ),
        (
            "the text path with an icon choice",
            with_identity(json!({"choice_of": "pick", "target_id": "detail/name"})),
            icon,
            "operation=pick expect_identity choice_of=identity_not_text",
        ),
    ] {
        refused(case, &task, &policy, token);
    }
}

// L4bT-T9 (L4b-T.2: REC-7 at admission): a pool template is a template target like any other. Under
// `ccorr_normed`, the package whose every template's threshold lies above its flat-field floor is
// admitted; the same package with `member.b`'s pool title at a threshold below that floor is
// refused where it is admitted, `resource_declaration_invalid` at that target's threshold, its
// detail naming the template's `flat_score_milli`.
#[test]
fn l4bt_a_pool_template_below_its_flat_field_floor_is_refused_at_admission() {
    let task = icon_task(expecting(5_000, "pick", pool()));
    let floored = |pool_threshold: f64| {
        let mut pack = pack(&task);
        pack["defaults"]["match_metric"] = json!("ccorr_normed");
        for target in pack["targets"].as_array_mut().expect("targets") {
            if target["type"] == "template" && target["id"] != "title/always" {
                target["threshold"] = if target["id"] == json!(title(B)) {
                    json!(pool_threshold)
                } else {
                    json!(0.99)
                };
            }
        }
        pack
    };
    admit_pack(&task, &icon_policy(), &floored(0.99))
        .unwrap_or_else(|error| panic!("every threshold above its floor: refused: {error}"));

    let pack = floored(0.5);
    let index = pack["targets"]
        .as_array()
        .expect("targets")
        .iter()
        .position(|target| target["id"] == json!(title(B)))
        .expect("the pool title of member.b");
    let error = match admit_pack(&task, &icon_policy(), &pack) {
        Ok(_) => panic!("a pool title below its floor: admitted"),
        Err(error) => error,
    };

    assert_eq!(
        error.code().as_str(),
        SharedCode::ResourceDeclarationInvalid.as_str(),
        "{error}"
    );
    let issue = error
        .declaration_issue()
        .expect("the refusal names its declaration");
    assert_eq!(issue.field_path, format!("/targets/{index}/threshold"));
    assert_eq!(issue.reason, ResourceDeclarationReason::InvalidValue);
    let detail = error.detail().unwrap_or_default();
    assert!(detail.contains("flat_score_milli="), "{detail:?}");
}

// L4bT-T10 (L4b-T.4, outcome golden): the template path's `absent` timeout is
// `selection_identity_mismatch` at `select_expect_identity` with `identity_state` `absent` and the
// expected ID. Its record form is exactly the golden below, and it reads back to the same record.
#[test]
fn l4bt_an_absent_mismatch_round_trips_with_its_code_stage_and_values() {
    let error = identity_mismatch(Some(MEMBERS[B]), scan::IdentityState::Absent, None);

    let record = serde_json::to_value(error.outcome()).expect("the outcome's record form");

    assert_eq!(
        record,
        json!({
            "code": "selection_identity_mismatch",
            "category": "error",
            "values": {"stage": "select_expect_identity", "identity_state": "absent",
                       "expected_identity": "member.b"}
        })
    );
    let read: Outcome = serde_json::from_value(record.clone()).expect("the record reads back");
    assert_eq!(
        serde_json::to_value(&read).expect("the read record's form"),
        record
    );
}
