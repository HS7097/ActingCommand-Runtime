// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #503 LIST-D (0.12.99; register §1.544; contracts/candidate-projection.md, "Budgets";
// contracts/selection-graph.md, "Rules"; contracts/component-interfaces.md, the `package` row 31):
// a candidate layout of either kind may declare `provider_evaluations`, 17 to 64, the most OCR and
// NN evaluations one projection of it may perform; absent, the budget is 16 as before. Every reader
// of the budget reads the layout's number: the check of one instance's evaluations before anything
// is read, the check of the actionable instances' evaluations, the admission bound of a
// `repeated_anchor` layout and the pack's rules. A layout without the member, in a pack beside one
// that declares it, is bounded and projected as before.
//
// Every expected value comes from those rules and the fixtures below, never from the code. The
// frame is 320 x 240: sixteen copies of one noise icon, the anchor, at the corners of four rows of
// four cards. A card reads two fields: its name (a 60 x 14 rectangle at (2, 12) from its icon,
// timeout 1000 ms) and its number (30 x 14 at (2, 30), timeout 500 ms). A scripted OCR provider
// answers a read of a card's name or number from the request's region, a detection with every
// card's name as a block inside its name rectangle, and records every request. The gate reads no
// clock.

use crate::*;
use actingcommand_contract::candidate_projection::CandidateProjection;
use actingcommand_contract::{CandidateFeature, ResourceDeclarationReason};
use serde_json::{Value, json};
use std::sync::Mutex;

const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const FRAME_WIDTH: u32 = 320;
const FRAME_HEIGHT: u32 = 240;
const LAYOUT: &str = "layout/cards";
// The second layout of test 6, which declares no budget of its own.
const OTHER_LAYOUT: &str = "layout/names";
const ICON: &str = "assets/card_icon.png";
const ICON_WIDTH: u32 = 12;
const ICON_HEIGHT: u32 = 10;
const ANCHOR: &str = "ui/card_icon";
const NAME: &str = "card/name";
const NUMBER: &str = "card/number";
const NAME_TIMEOUT_MS: u64 = 1000;
const NUMBER_TIMEOUT_MS: u64 = 500;
const SHARED_TIMEOUT_MS: u64 = 5000;
const CARDS: usize = 16;
// The name each card shows, in the order `top_to_bottom` numbers the cards.
const NAMES: [&str; CARDS] = [
    "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliet",
    "kilo", "lima", "mike", "november", "oscar", "papa",
];
// The pointer of the member in the first layout.
const MEMBER_POINTER: &str = "/candidate_layouts/0/provider_evaluations";

type Respond = dyn Fn(PackRect, OcrProviderBlocks) -> OcrProviderResult + Send + Sync;

// Answers every OCR request from its region and block mode and records each request.
struct ScriptedOcr {
    respond: Box<Respond>,
    requests: Mutex<Vec<(PackRect, OcrProviderBlocks)>>,
}

impl fmt::Debug for ScriptedOcr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ScriptedOcr")
    }
}

impl ScriptedOcr {
    fn new(
        respond: impl Fn(PackRect, OcrProviderBlocks) -> OcrProviderResult + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            respond: Box::new(respond),
            requests: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<(PackRect, OcrProviderBlocks)> {
        self.requests.lock().expect("request log").clone()
    }

    // The requests that ran the detector, and those that read a region on its own.
    fn detections_and_reads(&self) -> (usize, usize) {
        let requests = self.requests();
        let detections = requests
            .iter()
            .filter(|(_, blocks)| matches!(blocks, OcrProviderBlocks::Detect { .. }))
            .count();
        (detections, requests.len() - detections)
    }
}

impl VisionProvider for ScriptedOcr {
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
            .expect("request log")
            .push((request.region, request.blocks));
        Ok((self.respond)(request.region, request.blocks))
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

// One template asset at `path`.
#[derive(Debug)]
struct Asset {
    path: &'static str,
    png: Vec<u8>,
}

impl AssetResolver for Asset {
    fn read_asset(&self, path: &str) -> RecognitionPackResult<Vec<u8>> {
        if path == self.path {
            Ok(self.png.clone())
        } else {
            Err(RecognitionPackError::fatal(format!("no asset {path}")))
        }
    }
}

fn rect(x: i32, y: i32, width: i32, height: i32) -> PackRect {
    PackRect {
        x,
        y,
        width,
        height,
    }
}

// The top-left corner of card `card`'s icon: four columns 75 px apart, four rows 55 px apart.
fn corner(card: usize) -> (i32, i32) {
    let column = i32::try_from(card % 4).expect("a column");
    let row = i32::try_from(card / 4).expect("a row");
    (10 + 75 * column, 10 + 55 * row)
}

// The rectangle card `card`'s name is read in.
fn name_rect(card: usize) -> PackRect {
    let (x, y) = corner(card);
    rect(x + 2, y + 12, 60, 14)
}

// The rectangle card `card`'s number is read in.
fn number_rect(card: usize) -> PackRect {
    let (x, y) = corner(card);
    rect(x + 2, y + 30, 30, 14)
}

// The number card `card` shows.
fn number(card: usize) -> i64 {
    i64::try_from(3 * card + 1).expect("a number")
}

// The business ID of the name card `card` shows.
fn member_id(card: usize) -> String {
    format!("member-{card:02}")
}

fn block(text: &str, at: PackRect) -> OcrProviderTextBlock {
    OcrProviderTextBlock {
        text: text.to_owned(),
        rect: at,
        confidence: Some(0.9),
    }
}

// A detection's answer of `blocks`.
fn detected(blocks: Vec<OcrProviderTextBlock>) -> OcrProviderResult {
    OcrProviderResult {
        ppocr_diagnostics: Vec::new(),
        text: blocks
            .iter()
            .map(|served| served.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        blocks,
        confidence: Some(0.9),
        detection: None,
    }
}

// The cards' provider: a detection answers every card's name as a block inside its name rectangle;
// a read of a card's name rectangle answers its name, of its number rectangle its number; any other
// region reads empty text.
fn cards_ocr() -> Arc<ScriptedOcr> {
    ScriptedOcr::new(|region, blocks| {
        if matches!(blocks, OcrProviderBlocks::Detect { .. }) {
            return detected(
                (0..CARDS)
                    .map(|card| {
                        let name = name_rect(card);
                        block(NAMES[card], rect(name.x + 4, name.y + 2, 40, 10))
                    })
                    .collect(),
            );
        }
        let text = (0..CARDS)
            .find_map(|card| {
                if region == name_rect(card) {
                    Some(NAMES[card].to_owned())
                } else if region == number_rect(card) {
                    Some(number(card).to_string())
                } else {
                    None
                }
            })
            .unwrap_or_default();
        OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text,
            blocks: Vec::new(),
            confidence: Some(0.9),
            detection: None,
        }
    })
}

// A 12 x 10 icon of deterministic gray noise in 1..=255: a shifted copy correlates with it near
// zero.
fn icon() -> Vec<u8> {
    let mut state = 0x0503_1d00_0b0b_0001_u64;
    (0..ICON_WIDTH * ICON_HEIGHT)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            1 + ((state >> 33) % 255) as u8
        })
        .collect()
}

// The frame: gray 30, the icon at each card's corner.
fn cards_scene() -> Scene {
    let marker = icon();
    let mut pixels = vec![30_u8; (FRAME_WIDTH * FRAME_HEIGHT * 3) as usize];
    for card in 0..CARDS {
        let (x, y) = corner(card);
        for row in 0..ICON_HEIGHT {
            for column in 0..ICON_WIDTH {
                let value = marker[(row * ICON_WIDTH + column) as usize];
                let at = (((y as u32 + row) * FRAME_WIDTH + x as u32 + column) * 3) as usize;
                pixels[at..at + 3].copy_from_slice(&[value; 3]);
            }
        }
    }
    Scene::from_rgb8(FRAME_WIDTH, FRAME_HEIGHT, &pixels).expect("the frame")
}

// An OCR target of `width` x `height` in `languages`; a feature places it at its card.
fn ocr(id: &str, width: i32, height: i32, timeout_ms: u64, languages: &[&str]) -> Value {
    json!({
        "type": "ocr", "id": id, "region": {"x": 0, "y": 0, "width": width, "height": height},
        "languages": languages, "timeout_ms": timeout_ms, "match_mode": "exact",
        "expected": ["unused"], "case_sensitive": false, "minimum_confidence": 0.5,
        "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256
    })
}

// The `member` identity of a card, read from its name rectangle: one entry for each name.
fn member() -> Value {
    let entries = (0..CARDS)
        .map(|card| json!({"id": member_id(card), "aliases": [NAMES[card]]}))
        .collect::<Vec<_>>();
    json!({
        "name": "member", "value": "identity", "target": NAME, "offset": {"x": 2, "y": 12},
        "identity": {"entries": entries, "recognition": {
            "kind": "ocr_aliases", "max_distance": 0, "minimum_margin": 1,
            "minimum_confidence_milli": 800
        }}
    })
}

// The `number` of a card, read from its number rectangle.
fn number_feature() -> Value {
    json!({
        "name": "number", "value": "ocr_integer", "target": NUMBER, "offset": {"x": 2, "y": 30},
        "integer": {"min": 0, "max": 999, "minimum_confidence_milli": 800}
    })
}

// The two fields of a card: two OCR evaluations an instance.
fn two_fields() -> Vec<Value> {
    vec![member(), number_feature()]
}

// The cards' `repeated_anchor` layout `id` over `features`, declaring `provider_evaluations` when
// given.
fn cards_layout(id: &str, features: Vec<Value>, evaluations: Option<Value>) -> Value {
    let mut layout = json!({
        "id": id, "page_id": "neutral/cards", "kind": "repeated_anchor", "anchor": ANCHOR,
        "max_instances": 16, "order": "top_to_bottom", "suppress_iou_milli": 0,
        "instance_rect": {"x": 0, "y": 0, "width": 70, "height": 50},
        "click": {"x": 20, "y": 0, "width": 40, "height": 10},
        "features": features
    });
    if let Some(evaluations) = evaluations {
        layout["provider_evaluations"] = evaluations;
    }
    layout
}

// The layout of the two fields, declaring `provider_evaluations` when given.
fn two_field_layout(evaluations: Option<Value>) -> Value {
    cards_layout(LAYOUT, two_fields(), evaluations)
}

// The cards' pack of `layouts`; the number target reads `number_languages`.
fn cards_pack(layouts: Vec<Value>, number_languages: &[&str]) -> Value {
    json!({
        "schema_version": "0.7",
        "coordinate_space": {"width": FRAME_WIDTH, "height": FRAME_HEIGHT},
        "defaults": {"match_metric": "ccoeff_normed"},
        "targets": [
            {"type": "template", "id": ANCHOR, "template_path": ICON, "region": "full_frame",
             "threshold": 0.9},
            ocr(NAME, 60, 14, NAME_TIMEOUT_MS, &["en"]),
            ocr(NUMBER, 30, 14, NUMBER_TIMEOUT_MS, number_languages)
        ],
        "candidate_layouts": layouts
    })
}

// The pack of the one layout of the two fields.
fn two_field_pack(evaluations: Option<Value>) -> Value {
    cards_pack(vec![two_field_layout(evaluations)], &["en"])
}

fn load_evaluator(
    pack: &Value,
    provider: Arc<ScriptedOcr>,
) -> RecognitionPackResult<RecognitionEvaluator> {
    let rgb = icon()
        .iter()
        .flat_map(|value| [*value; 3])
        .collect::<Vec<_>>();
    RecognitionEvaluator::with_vision_provider(
        load_pack_from_json_str(&pack.to_string())?,
        Arc::new(Asset {
            path: ICON,
            png: encode_png(ICON_WIDTH, ICON_HEIGHT, &rgb),
        }),
        provider,
    )
}

// The admitted evaluator of `pack` over `provider`.
fn admitted(pack: &Value, provider: Arc<ScriptedOcr>) -> RecognitionEvaluator {
    load_evaluator(pack, provider).unwrap_or_else(|error| panic!("the pack: {}", error.message()))
}

// The admission bound of layout `layout_id` of `evaluator`.
fn bound(evaluator: &RecognitionEvaluator, layout_id: &str) -> u64 {
    evaluator
        .candidate_layout(layout_id)
        .expect("the layout")
        .maximum_provider_and_wait_ms(evaluator)
}

// Sixteen actionable cards, each with its name's `member` and its `number`.
fn assert_read_whole(projection: &CandidateProjection) {
    assert_eq!(projection.candidates().len(), CARDS);
    for (card, candidate) in projection.candidates().iter().enumerate() {
        assert!(candidate.actionable, "{candidate:?}");
        assert_eq!(candidate.features.len(), 2, "{candidate:?}");
        assert!(
            matches!(
                candidate.features.get("member"),
                Some(CandidateFeature::Identity { value, .. }) if *value == member_id(card)
            ),
            "{candidate:?}"
        );
        assert!(
            matches!(
                candidate.features.get("number"),
                Some(CandidateFeature::Integer { value, .. }) if *value == number(card)
            ),
            "{candidate:?}"
        );
    }
}

// Test 1 (ruling 1; the pack's reader): 17, 32 and 64 are admitted; 16, 0 and 65 are refused by
// the pack's rules at the member, with the diagnostic of `max_instances`, "must be 17..=64"; a
// value that is no unsigned integer (a string, a fraction, a negative number, `null`) is refused on
// the wire, `InvalidType` at the member. A member a reader's list of a layout's members does not
// hold is `UnknownField` at it: what a reader before this slice says of `provider_evaluations`.
#[test]
fn list_d_the_pack_takes_provider_evaluations_of_17_to_64_only() {
    for evaluations in [17, 32, 64] {
        admitted(&two_field_pack(Some(json!(evaluations))), cards_ocr());
    }
    for evaluations in [16, 0, 65] {
        let refused = load_evaluator(&two_field_pack(Some(json!(evaluations))), cards_ocr())
            .expect_err("a budget outside 17..=64");
        assert!(
            refused
                .message()
                .contains(&format!("{MEMBER_POINTER} must be 17..=64")),
            "{evaluations}: {}",
            refused.message()
        );
    }
    let wire = |pack: Value, pointer: &str, reason: ResourceDeclarationReason| {
        let Err(error) = load_pack_from_json_str(&pack.to_string()) else {
            panic!("{pointer}: the wire takes it");
        };
        let issue = error
            .declaration_issue()
            .unwrap_or_else(|| panic!("{pointer}: no declaration issue: {}", error.message()));
        assert_eq!(issue.field_path, pointer, "{}", error.message());
        assert_eq!(issue.reason, reason, "{}", error.message());
    };
    for written in [json!("32"), json!(32.5), json!(-1), Value::Null] {
        wire(
            two_field_pack(Some(written)),
            MEMBER_POINTER,
            ResourceDeclarationReason::InvalidType,
        );
    }
    let mut unknown = two_field_pack(Some(json!(32)));
    unknown["candidate_layouts"][0]["evaluation_budget"] = json!(32);
    wire(
        unknown,
        "/candidate_layouts/0/evaluation_budget",
        ResourceDeclarationReason::UnknownField,
    );
}

// Test 2 (the case it is for; rulings 1 and 2): sixteen actionable cards of two OCR fields are 32
// evaluations. Without the member the projection is refused before any read, `count` 32 and
// `limit_count` 16, as before; with `provider_evaluations` 32 it reads all 32 and projects the
// sixteen cards, each with its two features; with 31 it is refused, `count` 32 and `limit_count` 31.
#[test]
fn list_d_sixteen_cards_of_two_fields_project_with_a_budget_of_32_and_only_with_it() {
    let scene = cards_scene();
    for (evaluations, limit) in [(None, 16), (Some(json!(31)), 31)] {
        let provider = cards_ocr();
        let evaluator = admitted(&two_field_pack(evaluations), provider.clone());
        let over = evaluator
            .scene_context(&scene)
            .project_candidates(LAYOUT)
            .expect_err("32 OCR evaluations above the budget");
        assert_eq!(over.code().as_str(), "candidate_projection_budget_exceeded");
        assert_eq!(over.item(), "provider_evaluations");
        assert_eq!(
            over.detail(),
            format!("layout_id=layout/cards count=32 limit_count={limit}")
        );
        assert!(provider.requests().is_empty(), "{limit}: read nothing");
    }

    let provider = cards_ocr();
    let evaluator = admitted(&two_field_pack(Some(json!(32))), provider.clone());
    let projection = evaluator
        .scene_context(&scene)
        .project_candidates(LAYOUT)
        .unwrap_or_else(|failure| panic!("the cards project: {}", failure.detail()));
    assert_eq!(provider.detections_and_reads(), (0, 32));
    assert_read_whole(&projection);
}

// Test 3 (ruling 3): the admission bound of that layout. One card's own reads take 1000 + 500 ms;
// without the member the bound counts the eight cards the budget of 16 lets one projection read,
// 12 000 ms, as before; with 32 the sixteen cards, 24 000 ms; with 31 fifteen, 22 500 ms.
#[test]
fn list_d_the_admission_bound_counts_the_cards_the_layouts_budget_lets_one_projection_read() {
    let per_card = NAME_TIMEOUT_MS + NUMBER_TIMEOUT_MS;
    for (evaluations, cards) in [(None, 8), (Some(json!(32)), 16), (Some(json!(31)), 15)] {
        let evaluator = admitted(&two_field_pack(evaluations.clone()), cards_ocr());
        assert_eq!(
            bound(&evaluator, LAYOUT),
            cards * per_card,
            "{evaluations:?}"
        );
    }
    let today = admitted(&two_field_pack(None), cards_ocr());
    assert_eq!(bound(&today, LAYOUT), 12_000);
}

// The pack of one `fixed_slots` layout of `slots` slots in a row, each reading one OCR label at an
// offset, so one projection evaluates the label once for each slot, declaring `evaluations`.
fn slots_pack(slots: usize, evaluations: u64) -> Value {
    let slot = |index: usize| {
        let x = i32::try_from(16 * index).expect("a slot's x");
        json!({"rect": {"x": x, "y": 200, "width": 16, "height": 12},
               "click": {"x": x + 2, "y": 202, "width": 8, "height": 6},
               "targets": {}})
    };
    json!({
        "schema_version": "0.7",
        "coordinate_space": {"width": FRAME_WIDTH, "height": FRAME_HEIGHT},
        "defaults": {"match_metric": "ccoeff_normed"},
        "targets": [ocr("slot/label", 8, 6, NAME_TIMEOUT_MS, &["en"])],
        "candidate_layouts": [{
            "id": "layout/slots", "page_id": "neutral/cards", "kind": "fixed_slots",
            "features": [{"name": "label", "value": "passed", "target": "slot/label",
                          "offset": {"x": 2, "y": 2}}],
            "slots": (0..slots).map(slot).collect::<Vec<_>>(),
            "provider_evaluations": evaluations
        }]
    })
}

// Test 4 (ruling 2): one instance's own evaluations above the layout's declared budget. Eighteen
// slots of one label are 18 evaluations of one projection: with the budget 17 the pack's rules
// refuse the layout with the layout's number in the text; with 18 it is admitted, a `fixed_slots`
// layout taking the member too. The producer's own check, before anything is read, holds the same
// count to the layout's budget: on that admitted pack with its budget lowered to 17 behind the
// rules' back (only this crate's test can reach the pack an evaluator holds), the projection is
// refused, the layout's number in the text, and nothing is read.
#[test]
fn list_d_one_instance_above_the_declared_budget_is_refused_at_validation_and_before_any_read() {
    let refused = load_evaluator(&slots_pack(18, 17), cards_ocr())
        .expect_err("18 evaluations above a budget of 17");
    assert!(
        refused.message().contains(
            "/candidate_layouts/0/slots: 18 OCR and NN evaluations exceed the 17-evaluation projection budget"
        ),
        "{}",
        refused.message()
    );

    let provider = cards_ocr();
    let mut evaluator = admitted(&slots_pack(18, 18), provider.clone());
    evaluator.pack.candidate_layouts[0].provider_evaluations = Some(17);
    let over = evaluator
        .scene_context(&cards_scene())
        .project_candidates("layout/slots")
        .expect_err("18 evaluations above a budget of 17");
    assert_eq!(over.code().as_str(), "candidate_projection_budget_exceeded");
    assert_eq!(over.item(), "provider_evaluations");
    assert_eq!(
        over.detail(),
        "layout 'layout/slots' needs 18 OCR and NN evaluations above the 17-evaluation budget"
    );
    assert!(provider.requests().is_empty());
}

// The cards' layout of the two fields with a shared read over the whole frame, declaring
// `provider_evaluations` when given. The read gives the name its text; the number, read in other
// languages, is read on its own.
fn shared_pack(evaluations: Option<Value>) -> Value {
    let mut layout = two_field_layout(evaluations);
    layout["shared_ocr"] = json!({
        "region": "full_frame", "languages": ["en"], "timeout_ms": SHARED_TIMEOUT_MS,
        "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256,
        "blocks": "detect", "line_merge": {"gap_factor": 0}
    });
    cards_pack(vec![layout], &["en", "digits"])
}

// Test 5 (rulings 2 and 3): a layout with a shared read and the member. The read counts once a
// frame and each card's number once: 16 + 1 = 17. With `provider_evaluations` 17 the projection
// makes one detection and sixteen reads and reads every card whole, and the bound takes the read's
// timeout once and the numbers of the sixteen cards the 16 evaluations left after the read allow,
// 5000 + 16 x 500 ms; without the member the projection is refused, `count` 17 and `limit_count`
// 16, and the bound takes fifteen numbers, 5000 + 15 x 500 ms.
#[test]
fn list_d_a_shared_read_counts_once_and_the_cards_get_what_is_left_of_the_layouts_budget() {
    let scene = cards_scene();
    let provider = cards_ocr();
    let raised = admitted(&shared_pack(Some(json!(17))), provider.clone());
    assert_eq!(
        bound(&raised, LAYOUT),
        SHARED_TIMEOUT_MS + 16 * NUMBER_TIMEOUT_MS
    );
    let projection = raised
        .scene_context(&scene)
        .project_candidates(LAYOUT)
        .unwrap_or_else(|failure| panic!("the cards project: {}", failure.detail()));
    assert_eq!(provider.detections_and_reads(), (1, 16));
    assert_read_whole(&projection);

    let provider = cards_ocr();
    let today = admitted(&shared_pack(None), provider.clone());
    assert_eq!(
        bound(&today, LAYOUT),
        SHARED_TIMEOUT_MS + 15 * NUMBER_TIMEOUT_MS
    );
    let over = today
        .scene_context(&scene)
        .project_candidates(LAYOUT)
        .expect_err("17 evaluations above the budget of 16");
    assert_eq!(over.item(), "provider_evaluations");
    assert_eq!(
        over.detail(),
        "layout_id=layout/cards count=17 limit_count=16"
    );
    assert!(provider.requests().is_empty());
}

// Test 6 (ruling 5): two layouts in one pack, the cards of two fields with the budget 32 and the
// cards' names alone without the member. The second's bound (sixteen names, 16 000 ms) and its
// projection, byte for byte, and the reads it makes are those of a pack that holds it alone, as
// main bounds and projects it; the first projects its sixteen cards whole beside it.
#[test]
fn list_d_a_layout_without_the_member_beside_one_with_it_is_bounded_and_projected_as_before() {
    let scene = cards_scene();
    let names_layout = || cards_layout(OTHER_LAYOUT, vec![member()], None);
    let both = cards_pack(
        vec![two_field_layout(Some(json!(32))), names_layout()],
        &["en"],
    );
    let alone = cards_pack(vec![names_layout()], &["en"]);

    let mut projections = Vec::new();
    for pack in [&both, &alone] {
        let provider = cards_ocr();
        let evaluator = admitted(pack, provider.clone());
        assert_eq!(bound(&evaluator, OTHER_LAYOUT), 16 * NAME_TIMEOUT_MS);
        let projection = evaluator
            .scene_context(&scene)
            .project_candidates(OTHER_LAYOUT)
            .unwrap_or_else(|failure| panic!("the names project: {}", failure.detail()));
        assert_eq!(projection.candidates().len(), CARDS);
        assert_eq!(provider.detections_and_reads(), (0, 16));
        projections.push(serde_json::to_string(&projection).expect("the projection's JSON"));
    }
    assert_eq!(projections[0], projections[1]);

    let provider = cards_ocr();
    let evaluator = admitted(&both, provider.clone());
    let projection = evaluator
        .scene_context(&scene)
        .project_candidates(LAYOUT)
        .unwrap_or_else(|failure| panic!("the cards project: {}", failure.detail()));
    assert_read_whole(&projection);
    assert_eq!(provider.detections_and_reads(), (0, 32));
}

// An uncompressed RGB PNG.
fn encode_png(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    let mut scanlines = Vec::new();
    for row in pixels.chunks((width * 3) as usize) {
        scanlines.push(0);
        scanlines.extend_from_slice(row);
    }
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &header);
    let mut zlib = vec![0x78, 0x01];
    let blocks = scanlines.chunks(65_535).collect::<Vec<_>>();
    for (index, block) in blocks.iter().enumerate() {
        zlib.push(u8::from(index + 1 == blocks.len()));
        let length = block.len() as u16;
        zlib.extend_from_slice(&length.to_le_bytes());
        zlib.extend_from_slice(&(!length).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    let (mut a, mut b) = (1_u32, 0_u32);
    for byte in &scanlines {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    png
}

fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    png.extend_from_slice(kind);
    png.extend_from_slice(data);
    let mut crc = 0xffff_ffff_u32;
    for byte in kind.iter().chain(data) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    png.extend_from_slice(&(!crc).to_be_bytes());
}
