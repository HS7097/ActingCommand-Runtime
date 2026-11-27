// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 SEL-2L-b (register §1.480; contracts/selection-graph.md, "Shared OCR read";
// contracts/component-interfaces.md, the `package` row 20; the coordinator's work orders
// rd-1118/brief-463-sel2l-b-anchor-shared-ocr.md and rd-1118/brief-463-sel2l-b-finish.md, with its
// ruling on the order of blocks): a `repeated_anchor` layout may declare `shared_ocr`, whose region
// covers the layout's readable band. On the frame the detection runs once over it; a block belongs
// to the actionable instance whose name rectangle holds its centre; an instance's blocks are ordered
// by top, then left, and joined with line feeds, as a slot's are; an instance without a block reads
// empty text; a block in no instance is dropped; an instance the band cuts is not read. The
// projection counts the shared read once a frame, whatever the instances. A layout without the
// member projects as before, and a package that needs `package` 20 is within this build's range.
//
// Every expected value comes from those rules and the fixtures below, never from the code. The
// frame is 320 x 240: nine copies of one noise icon, the anchor, at the corners of three rows of
// three cards; the readable band ends above the third row. A card's name rectangle is 92 x 36 at
// (4, 12) from its icon. A scripted OCR provider answers with the blocks each test gives and
// records every request. The gate reads no clock.

use crate::codes::RecognitionPackCode;
use crate::*;
use actingcommand_contract::candidate_projection::CandidateProjection;
use actingcommand_contract::{CandidateFeature, package_edge_holds};
use serde_json::{Value, json};
use std::sync::Mutex;

const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const FRAME_WIDTH: u32 = 320;
const FRAME_HEIGHT: u32 = 240;
const LAYOUT: &str = "layout/cards";
const ICON: &str = "assets/card_icon.png";
const ICON_WIDTH: u32 = 12;
const ICON_HEIGHT: u32 = 10;
const ANCHOR: &str = "ui/card_icon";
const NAME: &str = "card/name";
// The readable band: the first two rows of cards; the third reaches below it.
const BAND: PackRect = PackRect {
    x: 0,
    y: 10,
    width: 320,
    height: 200,
};
const DETECT: OcrProviderBlocks = OcrProviderBlocks::Detect {
    line_merge_gap_factor: Some(0),
};
// The icon's top-left corner on each card, in the order `top_to_bottom` numbers them.
const CARDS: [(i32, i32); 9] = [
    (20, 20),
    (120, 20),
    (220, 20),
    (20, 100),
    (120, 100),
    (220, 100),
    (20, 180),
    (120, 180),
    (220, 180),
];
// The cards inside the band: the first two rows.
const ACTIONABLE: usize = 6;
// The admission's message for a shared read that does not cover the band.
const SHORT_OF_THE_BAND: &str = "/candidate_layouts/0/shared_ocr/region: the shared read of a repeated_anchor layout covers its readable_band, or the whole frame when it declares none";
// The `package` revision a shared read of a `repeated_anchor` layout needs, and the declaration
// this build copies into its manifests.
const SHARED_READ_REVISION: u64 = 20;
const OWN_DECLARATION: &str =
    include_str!("../../../distribution/windows/component-interfaces.json");

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

fn rect_json(rect: PackRect) -> Value {
    json!({"x": rect.x, "y": rect.y, "width": rect.width, "height": rect.height})
}

// The name rectangle of the card whose icon stands at `card`.
fn name_rect(card: (i32, i32)) -> PackRect {
    rect(card.0 + 4, card.1 + 12, 92, 36)
}

fn block(text: &str, rect: PackRect) -> OcrProviderTextBlock {
    OcrProviderTextBlock {
        text: text.to_owned(),
        rect,
        confidence: Some(0.9),
    }
}

// A block `text` of `width` x `height` at `(dx, dy)` inside the name rectangle of card `card`.
fn on(card: usize, text: &str, dx: i32, dy: i32, width: i32, height: i32) -> OcrProviderTextBlock {
    let name = name_rect(CARDS[card]);
    block(text, rect(name.x + dx, name.y + dy, width, height))
}

fn result(
    blocks: Vec<OcrProviderTextBlock>,
    detection: Option<OcrProviderDetection>,
) -> OcrProviderResult {
    OcrProviderResult {
        ppocr_diagnostics: Vec::new(),
        text: blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        blocks,
        confidence: Some(0.9),
        detection,
    }
}

// A provider that answers every request with `blocks`.
fn reading(blocks: Vec<OcrProviderTextBlock>) -> Arc<ScriptedOcr> {
    ScriptedOcr::new(move |_, _| result(blocks.clone(), None))
}

// A 12 x 10 icon of deterministic gray noise in 1..=255: a shifted copy correlates with it near
// zero.
fn icon() -> Vec<u8> {
    let mut state = 0x0463_5e12_0b0b_0001_u64;
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
fn scene() -> Scene {
    let icon = icon();
    let mut pixels = vec![30_u8; (FRAME_WIDTH * FRAME_HEIGHT * 3) as usize];
    for (x, y) in CARDS {
        for row in 0..ICON_HEIGHT {
            for column in 0..ICON_WIDTH {
                let value = icon[(row * ICON_WIDTH + column) as usize];
                let at = (((y as u32 + row) * FRAME_WIDTH + x as u32 + column) * 3) as usize;
                pixels[at..at + 3].copy_from_slice(&[value; 3]);
            }
        }
    }
    Scene::from_rgb8(FRAME_WIDTH, FRAME_HEIGHT, &pixels).expect("the frame")
}

// An OCR target of `width` x `height`; a feature places it at its card.
fn ocr(id: &str, width: i32, height: i32) -> Value {
    json!({
        "type": "ocr", "id": id, "region": {"x": 0, "y": 0, "width": width, "height": height},
        "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact", "expected": ["unused"],
        "case_sensitive": false, "minimum_confidence": 0.5, "model_ref": "PP-OCRv6_medium",
        "model_sha256": MODEL_SHA256
    })
}

// The shared read over `region`.
fn shared_ocr(region: Value) -> Value {
    json!({
        "region": region, "languages": ["en"], "timeout_ms": 5000,
        "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256,
        "blocks": "detect", "line_merge": {"gap_factor": 0}
    })
}

// The `member` identity of a card over `names`, `(id, alias)` each, read from its name rectangle.
fn member(names: &[(&str, &str)]) -> Value {
    let entries = names
        .iter()
        .map(|(id, alias)| json!({"id": id, "aliases": [alias]}))
        .collect::<Vec<_>>();
    json!({
        "name": "member", "value": "identity", "target": NAME, "offset": {"x": 4, "y": 12},
        "identity": {"entries": entries, "recognition": {
            "kind": "ocr_aliases", "max_distance": 0, "minimum_margin": 1,
            "minimum_confidence_milli": 800
        }}
    })
}

// The cards' pack: the icon anchors a `repeated_anchor` layout of `features`, with the readable
// band `band` and the shared read `shared` when given.
fn cards_pack(features: Vec<Value>, band: Option<PackRect>, shared: Option<Value>) -> Value {
    let mut layout = json!({
        "id": LAYOUT, "page_id": "neutral/cards", "kind": "repeated_anchor", "anchor": ANCHOR,
        "max_instances": 12, "order": "top_to_bottom", "suppress_iou_milli": 0,
        "instance_rect": {"x": 0, "y": 0, "width": 96, "height": 70},
        "click": {"x": 0, "y": 50, "width": 96, "height": 20},
        "features": features
    });
    if let Some(band) = band {
        layout["readable_band"] = rect_json(band);
    }
    if let Some(shared) = shared {
        layout["shared_ocr"] = shared;
    }
    json!({
        "schema_version": "0.7",
        "coordinate_space": {"width": FRAME_WIDTH, "height": FRAME_HEIGHT},
        "defaults": {"match_metric": "ccoeff_normed"},
        "targets": [
            {"type": "template", "id": ANCHOR, "template_path": ICON, "region": "full_frame",
             "threshold": 0.9},
            ocr(NAME, 92, 36),
            ocr("card/tag", 40, 16),
            ocr("card/note", 40, 16)
        ],
        "candidate_layouts": [layout]
    })
}

// The cards' pack with the shared read over the band.
fn shared_cards(features: Vec<Value>) -> Value {
    cards_pack(features, Some(BAND), Some(shared_ocr(rect_json(BAND))))
}

fn evaluator(
    pack: Value,
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

// The projection of the cards on `scene`, and the text each actionable card's name took, in card
// order.
fn project(evaluator: &RecognitionEvaluator, scene: &Scene) -> (CandidateProjection, Vec<String>) {
    let mut texts = Vec::new();
    let projection = evaluator
        .scene_context(scene)
        .project_candidates_with_samples(LAYOUT, &[scene], &mut |target, result, _, _| {
            if target == NAME
                && let Ok(evaluation) = result
            {
                texts.push(
                    evaluation
                        .ocr
                        .as_ref()
                        .expect("the name's OCR evidence")
                        .text
                        .clone(),
                );
            }
            Ok(())
        })
        .expect("the cards project");
    (projection, texts)
}

// The `member` identity of each actionable card, `None` where it is Unknown.
fn members(projection: &CandidateProjection) -> Vec<Option<String>> {
    projection.candidates()[..ACTIONABLE]
        .iter()
        .map(|candidate| match candidate.features.get("member") {
            Some(CandidateFeature::Identity { value, .. }) => Some(value.clone()),
            Some(CandidateFeature::Unknown { .. }) => None,
            other => panic!("{} has the member {other:?}", candidate.id),
        })
        .collect()
}

// Nine cards, of which the third row, past the band, is not actionable and carries no features.
fn assert_third_row_unread(projection: &CandidateProjection) {
    assert_eq!(projection.candidates().len(), CARDS.len());
    for (index, candidate) in projection.candidates().iter().enumerate() {
        assert_eq!(candidate.actionable, index < ACTIONABLE, "{candidate:?}");
        if index >= ACTIONABLE {
            assert!(candidate.features.is_empty(), "{candidate:?}");
        }
    }
}

fn names(ids: &[&str]) -> Vec<Option<String>> {
    ids.iter()
        .map(|id| (!id.is_empty()).then(|| (*id).to_owned()))
        .collect()
}

// The names of the first test: two lines, one line centred across the middle, both kinds in each
// row of the grid.
fn grid_names() -> [(&'static str, &'static str); 8] {
    [
        ("alpha", "ALPHA"),
        ("bravo", "BRAVO"),
        ("alpha-bravo", "ALPHA BRAVO"),
        ("charlie", "CHARLIE"),
        ("delta-echo", "DELTA ECHO"),
        ("foxtrot", "FOXTROT"),
        ("golf-hotel", "GOLF HOTEL"),
        ("india", "INDIA"),
    ]
}

// Their blocks: a two-line name's lines at 2..17 and 19..34 of the 36-pixel name rectangle, a
// one-line name at 10..26, across its middle (18).
fn grid_blocks() -> Vec<OcrProviderTextBlock> {
    vec![
        on(0, "ALPHA", 4, 2, 60, 15),
        on(0, "BRAVO", 4, 19, 40, 15),
        on(1, "CHARLIE", 6, 10, 80, 16),
        on(2, "DELTA", 4, 2, 60, 15),
        on(2, "ECHO", 4, 19, 40, 15),
        on(3, "FOXTROT", 6, 10, 80, 16),
        on(4, "GOLF", 4, 2, 40, 15),
        on(4, "HOTEL", 4, 19, 50, 15),
        on(5, "INDIA", 6, 10, 80, 16),
    ]
}

// Test 1: on a `repeated_anchor` layout with `shared_ocr`, a two-line name and a one-line name
// centred across the middle, both kinds in one row of the grid, are each read whole from one
// detection over the band.
#[test]
fn sel2l_two_line_and_one_line_names_in_one_grid_row_are_each_read_whole() {
    let provider = reading(grid_blocks());
    let evaluator = evaluator(shared_cards(vec![member(&grid_names())]), provider.clone())
        .expect("a repeated_anchor layout with a shared read over its band is admitted");
    let scene = scene();

    let (projection, texts) = project(&evaluator, &scene);

    assert_eq!(provider.requests(), [(BAND, DETECT)]);
    assert_eq!(
        texts,
        [
            "ALPHA\nBRAVO",
            "CHARLIE",
            "DELTA\nECHO",
            "FOXTROT",
            "GOLF\nHOTEL",
            "INDIA"
        ]
    );
    assert_eq!(
        members(&projection),
        names(&[
            "alpha-bravo",
            "charlie",
            "delta-echo",
            "foxtrot",
            "golf-hotel",
            "india"
        ])
    );
    assert_third_row_unread(&projection);
}

// Test 2: a neighbour's text close by goes to the neighbour; a block that lies across the name
// rectangles of two cards goes to the card that holds its centre; a block in no card is dropped; a
// card without a block reads empty text, so its identity is Unknown; the cut row is not read, and
// the block over its name is given to no card.
#[test]
fn sel2l_each_block_goes_to_the_card_that_holds_its_centre_and_a_cut_row_is_not_read() {
    let blocks = vec![
        // Card 0's name rectangle is x 24..116: its text ends at its right edge.
        on(0, "ALPHA", 52, 10, 40, 16),
        // Card 1's (x 124..216) starts 9 pixels to its right: its text, from x 125 on.
        on(1, "CHARLIE", 1, 10, 40, 16),
        // x 206..246 over card 1's rectangle and card 2's (x 224..316); its centre, x 226, is in
        // card 2's.
        block("DELTA", rect(206, 42, 40, 16)),
        // Card 3 has no block.
        on(4, "ECHO", 6, 10, 40, 16),
        on(5, "FOXTROT", 6, 10, 60, 16),
        // Between the rows (centre 165, 90): in no card.
        block("GOLF", rect(150, 85, 30, 10)),
        // Over card 6's name, in the band (y 194..208) but on a card the band cuts.
        on(6, "HOTEL", 4, 2, 40, 14),
    ];
    let provider = reading(blocks);
    let identities = [
        ("alpha", "ALPHA"),
        ("charlie", "CHARLIE"),
        ("delta", "DELTA"),
        ("echo", "ECHO"),
        ("foxtrot", "FOXTROT"),
        ("golf", "GOLF"),
        ("hotel", "HOTEL"),
    ];
    let evaluator = evaluator(shared_cards(vec![member(&identities)]), provider.clone())
        .expect("a repeated_anchor layout with a shared read over its band is admitted");
    let scene = scene();

    let (projection, texts) = project(&evaluator, &scene);

    assert_eq!(provider.requests(), [(BAND, DETECT)]);
    assert_eq!(texts, ["ALPHA", "CHARLIE", "DELTA", "", "ECHO", "FOXTROT"]);
    assert_eq!(
        members(&projection),
        names(&["alpha", "charlie", "delta", "", "echo", "foxtrot"])
    );
    assert_third_row_unread(&projection);
}

// Test 3, as the coordinator ruled: blocks are ordered by their top, then their left, so of two
// blocks of one line whose tops differ by a pixel the higher comes first, also where it stands to
// the right. That is the order of a fixed slot's blocks too, and this slice keeps it; an author
// who sees one line come back as two blocks raises `line_merge`'s gap factor, so that the provider
// joins them before the order is applied (the scripted provider joins nothing).
#[test]
fn sel2l_two_blocks_of_one_line_a_pixel_apart_are_ordered_by_top_then_left() {
    let blocks = vec![
        // Card 0: the right block one pixel higher.
        on(0, "LEFT", 4, 10, 40, 16),
        on(0, "RIGHT", 48, 9, 40, 16),
        // Card 1: the left block one pixel higher.
        on(1, "LEFT", 4, 9, 40, 16),
        on(1, "RIGHT", 48, 10, 40, 16),
    ];
    let provider = reading(blocks);
    let identities = [("left-right", "LEFT RIGHT"), ("right-left", "RIGHT LEFT")];
    let evaluator = evaluator(shared_cards(vec![member(&identities)]), provider.clone())
        .expect("a repeated_anchor layout with a shared read over its band is admitted");
    let scene = scene();

    let (projection, texts) = project(&evaluator, &scene);

    assert_eq!(provider.requests(), [(BAND, DETECT)]);
    assert_eq!(texts, ["RIGHT\nLEFT", "LEFT\nRIGHT", "", "", "", ""]);
    assert_eq!(
        members(&projection),
        names(&["right-left", "left-right", "", "", "", ""])
    );
}

// Test 4: three OCR features a card on six actionable cards are 18 evaluations read on their own,
// above the budget of 16, and the projection is refused before any read; with the shared read
// over the band they are one evaluation, the one provider call, and the admission bound is the
// read's timeout once.
#[test]
fn sel2l_the_shared_read_is_one_evaluation_a_frame_whatever_the_cards() {
    let features = || {
        vec![
            member(&grid_names()),
            json!({"name": "tag", "value": "passed", "target": "card/tag",
                   "offset": {"x": 4, "y": 50}}),
            json!({"name": "note", "value": "passed", "target": "card/note",
                   "offset": {"x": 50, "y": 50}}),
        ]
    };
    let scene = scene();

    let provider = reading(grid_blocks());
    let alone = evaluator(cards_pack(features(), Some(BAND), None), provider.clone())
        .expect("three OCR features of one card fit the budget");
    // Three reads of 1000 ms for each of the five cards the budget lets one projection read.
    assert_eq!(
        alone
            .candidate_layout(LAYOUT)
            .expect("the layout")
            .maximum_provider_and_wait_ms(&alone),
        15_000
    );
    let over = alone
        .scene_context(&scene)
        .project_candidates(LAYOUT)
        .expect_err("18 OCR evaluations exceed the budget of 16");
    assert_eq!(over.code().as_str(), "candidate_projection_budget_exceeded");
    assert_eq!(over.item(), "provider_evaluations");
    assert_eq!(
        over.detail(),
        "layout_id=layout/cards count=18 limit_count=16"
    );
    assert!(provider.requests().is_empty());

    let provider = reading(grid_blocks());
    let shared = evaluator(shared_cards(features()), provider.clone())
        .expect("the shared read serves every feature");
    assert_eq!(
        shared
            .candidate_layout(LAYOUT)
            .expect("the layout")
            .maximum_provider_and_wait_ms(&shared),
        5_000,
        "one provider call: the shared read's timeout"
    );

    let (projection, texts) = project(&shared, &scene);

    assert_eq!(provider.requests(), [(BAND, DETECT)]);
    assert_eq!(texts.len(), ACTIONABLE);
    assert_eq!(
        members(&projection),
        names(&[
            "alpha-bravo",
            "charlie",
            "delta-echo",
            "foxtrot",
            "golf-hotel",
            "india"
        ])
    );
    for candidate in &projection.candidates()[..ACTIONABLE] {
        for feature in ["tag", "note"] {
            assert!(
                matches!(
                    candidate.features.get(feature),
                    Some(CandidateFeature::Boolean { value: false, .. })
                ),
                "{} {feature}: no block, so not the expected text: {:?}",
                candidate.id,
                candidate.features.get(feature)
            );
        }
    }
}

// Test 5: a region that does not cover the band, or without a band the whole frame, is refused at
// its pointer when the pack is admitted, and `full_frame` or a rectangle that holds it is admitted;
// a detection that found more boxes than the provider's bound fails the projection with the
// existing truncation word.
#[test]
fn sel2l_a_region_short_of_the_band_is_refused_and_a_truncated_read_fails() {
    let features = || vec![member(&[("alpha", "ALPHA")])];
    for (band, region) in [
        // Below the band's top.
        (Some(BAND), rect_json(rect(0, 20, 320, 190))),
        // Left of its right edge.
        (Some(BAND), rect_json(rect(0, 10, 300, 200))),
        // Without a band, short of the whole frame.
        (None, rect_json(BAND)),
    ] {
        let refused = evaluator(
            cards_pack(features(), band, Some(shared_ocr(region.clone()))),
            reading(Vec::new()),
        )
        .expect_err("a region short of the band");
        assert!(
            refused.message().contains(SHORT_OF_THE_BAND),
            "{region}: {}",
            refused.message()
        );
    }
    for (band, region) in [
        (Some(BAND), json!("full_frame")),
        (None, json!("full_frame")),
        (None, rect_json(rect(0, 0, 320, 240))),
    ] {
        evaluator(
            cards_pack(features(), band, Some(shared_ocr(region.clone()))),
            reading(Vec::new()),
        )
        .unwrap_or_else(|error| panic!("{region}: {}", error.message()));
    }

    let provider = ScriptedOcr::new(|_, _| {
        result(
            grid_blocks(),
            Some(OcrProviderDetection {
                detected: 65,
                limit: 64,
                served: None,
            }),
        )
    });
    let evaluator = evaluator(shared_cards(features()), provider.clone())
        .expect("a repeated_anchor layout with a shared read over its band is admitted");
    let scene = scene();

    let failure = evaluator
        .scene_context(&scene)
        .project_candidates(LAYOUT)
        .expect_err("a card may lack its blocks");

    assert_eq!(failure.code().as_str(), "candidate_feature_failed");
    let outcome = failure
        .cause()
        .and_then(RecognitionPackError::outcome)
        .expect("the read's outcome");
    assert!(outcome.is(RecognitionPackCode::TextReadBlocksTruncated));
    assert_eq!(outcome.value("stage"), Some(&json!("text_read_shared")));
    assert_eq!(outcome.value("count"), Some(&json!(65)));
    assert_eq!(outcome.value("limit_count"), Some(&json!(64)));
    assert_eq!(provider.requests(), [(BAND, DETECT)]);
}

// Workflow #308 L1's fixture of `repeated_anchor` layouts (lib.rs, `repeated_anchor`), unchanged:
// an 8 x 6 RGB noise marker.
const MARKER: &str = "assets/marker.png";

fn marker() -> Vec<u8> {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut channel = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 56) as u8
    };
    (0..8 * 6 * 3).map(|_| channel()).collect()
}

// Its 96 x 72 frame: markers at (4, 10), (50, 10), (4, 40) and (60, 62), the open square beside
// the first.
fn rows_scene() -> Scene {
    let (width, height) = (96_u32, 72_u32);
    let mut pixels = [30_u8, 31, 32].repeat((width * height) as usize);
    let marker = marker();
    for (x, y) in [(4_u32, 10_u32), (50, 10), (4, 40), (60, 62)] {
        for row in 0..6_u32 {
            let from = (row * 8 * 3) as usize;
            let at = (((y + row) * width + x) * 3) as usize;
            pixels[at..at + 8 * 3].copy_from_slice(&marker[from..from + 8 * 3]);
        }
    }
    for y in 11..15_u32 {
        for x in 34..38_u32 {
            let at = ((y * width + x) * 3) as usize;
            pixels[at..at + 3].copy_from_slice(&[0, 200, 0]);
        }
    }
    Scene::from_rgb8(width, height, &pixels).expect("the rows frame")
}

// Its pack, whose layout declares no shared read.
fn rows_pack() -> Value {
    json!({
        "schema_version": "0.7",
        "coordinate_space": {"width": 96, "height": 72},
        "defaults": {"match_metric": "ccoeff_normed"},
        "targets": [
            {"type": "template", "id": "ui/row_marker", "template_path": MARKER,
             "region": "full_frame", "threshold": 0.95},
            {"type": "color", "id": "state/row_open",
             "region": {"x": 0, "y": 0, "width": 4, "height": 4}, "expected": [0, 200, 0]},
            {"type": "nn", "id": "model/row_kind", "region": "full_frame",
             "model_ref": "fixture-page-model", "model_sha256": "b".repeat(64),
             "candidate_labels": ["open", "closed"], "minimum_score": 0.75,
             "selection": "label", "expected_label": "open", "timeout_ms": 1000},
            {"type": "composite", "id": "check/row_ready", "mode": "all_of",
             "members": ["state/row_open", "ui/row_marker"]}
        ],
        "candidate_layouts": [{
            "id": "layout/rows", "page_id": "list_page", "kind": "repeated_anchor",
            "anchor": "ui/row_marker", "max_instances": 4, "order": "top_to_bottom",
            "instance_rect": {"x": -2, "y": -2, "width": 40, "height": 12},
            "click": {"x": 10, "y": 0, "width": 20, "height": 6},
            "features": [{"name": "open", "value": "passed", "target": "state/row_open",
                          "offset": {"x": 30, "y": 1}}]
        }]
    })
}

// Test 6: a layout without the member projects as before: L1's fixture keeps its candidates and
// the candidate-set hash its test pins. A package that needs `package` 20 is within this build's
// range at the installer's edge, which a range ending at 19 refuses; a bundle of before stays
// within both. Tests 1 to 5 admit such a layout's pack.
#[test]
fn sel2l_a_layout_without_the_member_projects_as_before_and_package_20_is_in_range() {
    let evaluator = RecognitionEvaluator::with_asset_resolver(
        load_pack_from_json_str(&rows_pack().to_string()).expect("the rows pack loads"),
        Arc::new(Asset {
            path: MARKER,
            png: encode_png(8, 6, &marker()),
        }),
    )
    .expect("the rows layout is admitted");
    let scene = rows_scene();

    let projection = evaluator
        .scene_context(&scene)
        .project_candidates("layout/rows")
        .expect("the rows project");

    let summary = projection
        .candidates()
        .iter()
        .map(|candidate| {
            let open = match candidate.features.get("open") {
                None => None,
                Some(CandidateFeature::Boolean { value, .. }) => Some(*value),
                Some(other) => panic!("unexpected feature {other:?}"),
            };
            (
                candidate.rect.x,
                candidate.rect.y,
                candidate.actionable,
                open,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        summary,
        [
            (2, 8, true, Some(true)),
            (48, 8, true, Some(false)),
            (2, 38, true, Some(false)),
            (58, 60, false, None),
        ]
    );
    assert_eq!(
        projection.candidate_set_sha256(),
        "69a3bdfe911c4871f42bbfa7f080253dac4f8416a286c4ac18435a457fb381c8"
    );

    let declared: Value =
        serde_json::from_str(OWN_DECLARATION).expect("the build's declaration is JSON");
    let own = match declared["speaks"]["package"].as_array().map(Vec::as_slice) {
        Some([min, max]) => [
            min.as_u64().expect("a minimum"),
            max.as_u64().expect("a maximum"),
        ],
        other => panic!("the build's package range: {other:?}"),
    };
    let needs = [SHARED_READ_REVISION, SHARED_READ_REVISION];
    // Workflow #463 SEL-GRID G2 (register §1.491) raises the build's range to 21, the lines of a
    // shared read's name boxes, Workflow #428 S168-LK (register §1.500) to 22, a linear step's
    // list reading, Workflow #455 G-SLx (register §1.501) to 23, a pack's variables beside an
    // irreversible capability, Workflow #477 CV-1 (register §1.507) to 24, a bundle that carries
    // a version 2 package declaration, Workflow #477 LR-1 (register §1.513) to 25, a linear
    // press step's count, Workflow #455 TK-1 (register §1.532) to 26, a variable only a rule
    // gives, Workflow #503 LIST-A (register §1.533) to 27, a list reading of the wanted keys only,
    // and Workflow #455 RR1 (register §1.535) to 28, a `goals.json` of version 3 and a root's
    // `goal-tables.json`; 20 stays within it.
    assert_eq!(own, [3, SHARED_READ_REVISION + 8]);
    assert!(package_edge_holds(own, needs));
    assert!(!package_edge_holds([3, SHARED_READ_REVISION - 1], needs));
    assert!(package_edge_holds(own, [3, SHARED_READ_REVISION - 1]));
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
