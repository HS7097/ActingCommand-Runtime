// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 SEL-GRID G1 (register §1.490; contracts/selection-graph.md, "Shared OCR read",
// "What the read recognizes"; the coordinator's work order
// rd-1119/brief-463-g1-shared-read-serves-name-boxes.md, its tests 1, 3, 5 and 8): before a frame's
// first shared read the kernel places the name boxes the read serves there, and the read sends
// them to the text provider, which recognizes and counts only the blocks whose centre lies in one.
// A frame with many more texts than names reads; served blocks over the cap still fail; a located
// text sends no box; a card the band cuts sends none.
//
// Every expected value comes from those rules and the fixtures below, never from the code. The
// frames are synthetic: gray 30 with one noise icon, the anchor, at the top-left corner of each
// card. A card is 96 x 70; its name box is 92 x 36 at (4, 12) from its icon; besides its name of
// one or two lines it shows six other texts (a cover print, a tag, a price, a limit, a buy key, a
// stock count), each centred outside the name box. A scripted provider does what the text
// provider's contract says: its detector finds every block it is given; when the request names
// served name boxes it keeps the blocks whose centre lies in one, by the kernel's own rule (the
// contract crate's, which the real provider applies too); it recognizes the first `limit` of the
// blocks it keeps and reports the detected and served counts; it records every request. The gate
// reads no clock.

use crate::*;
use actingcommand_contract::CandidateFeature;
use actingcommand_contract::candidate_projection::CandidateProjection;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Mutex;

const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const LAYOUT: &str = "layout/cards";
const ICON: &str = "assets/card_icon.png";
const ICON_WIDTH: u32 = 12;
const ICON_HEIGHT: u32 = 10;
const ANCHOR: &str = "ui/card_icon";
const NAME: &str = "card/name";
/// The model folder's `max_boxes` by default (`contracts/vision-model-folders.md`).
const MAX_BOXES: u32 = 64;
const DETECT: OcrProviderBlocks = OcrProviderBlocks::Detect {
    line_merge_gap_factor: Some(0),
};

/// Test 1's frame: ten cards in four columns, in rows of four, four and two.
const LIST_WIDTH: u32 = 420;
const LIST_HEIGHT: u32 = 260;
const LIST: [(i32, i32); 10] = [
    (10, 10),
    (110, 10),
    (210, 10),
    (310, 10),
    (10, 90),
    (110, 90),
    (210, 90),
    (310, 90),
    (10, 170),
    (110, 170),
];

/// Test 8's frame: sixteen cards in four rows of four; the readable band cuts the first row at its
/// top and the last at its bottom, so that eight cards are whole.
const GRID_WIDTH: u32 = 420;
const GRID_HEIGHT: u32 = 340;
const GRID_BAND: PackRect = PackRect {
    x: 0,
    y: 40,
    width: 420,
    height: 250,
};

/// What one request asked: its frame's size, its region, its block mode and its name boxes.
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    frame: (u32, u32),
    region: PackRect,
    blocks: OcrProviderBlocks,
    served: Vec<PackRect>,
}

/// The text provider of the module comment: `blocks` are what its detector finds on every request.
struct Provider {
    blocks: Vec<OcrProviderTextBlock>,
    limit: u32,
    seen: Mutex<Vec<Seen>>,
}

impl fmt::Debug for Provider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Provider")
    }
}

impl Provider {
    fn new(blocks: Vec<OcrProviderTextBlock>, limit: u32) -> Arc<Self> {
        Arc::new(Self {
            blocks,
            limit,
            seen: Mutex::new(Vec::new()),
        })
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().expect("request log").clone()
    }
}

impl VisionProvider for Provider {
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
        self.seen.lock().expect("request log").push(Seen {
            frame: (request.frame.width, request.frame.height),
            region: request.region,
            blocks: request.blocks,
            served: request.served.to_vec(),
        });
        // Between the merge and the cap: with name boxes, the blocks whose centre lies in one.
        let kept = self
            .blocks
            .iter()
            .filter(|block| {
                request.served.is_empty()
                    || request
                        .served
                        .iter()
                        .any(|name_box| super::center_is_within(block.rect, *name_box))
            })
            .cloned()
            .collect::<Vec<_>>();
        let detection = OcrProviderDetection {
            detected: count(self.blocks.len()),
            limit: self.limit,
            served: (!request.served.is_empty()).then(|| count(kept.len())),
        };
        let recognized = kept
            .into_iter()
            .take(usize::try_from(self.limit).expect("a cap"))
            .collect();
        Ok(result(recognized, Some(detection)))
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

fn count(blocks: usize) -> u32 {
    u32::try_from(blocks).expect("a count of blocks")
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

fn block(text: &str, rect: PackRect) -> OcrProviderTextBlock {
    OcrProviderTextBlock {
        text: text.to_owned(),
        rect,
        confidence: Some(0.9),
    }
}

/// The name box of the card whose icon stands at `card`.
fn name_box(card: (i32, i32)) -> PackRect {
    rect(card.0 + 4, card.1 + 12, 92, 36)
}

/// A card's name: one line at 22..38 of the card, two lines at 14..29 and 31..46; each line's
/// centre lies inside the name box (12..48).
fn name_blocks(card: (i32, i32), lines: &[&str]) -> Vec<OcrProviderTextBlock> {
    let (x, y) = card;
    match lines {
        [line] => vec![block(line, rect(x + 10, y + 22, 70, 16))],
        _ => lines
            .iter()
            .enumerate()
            .map(|(index, line)| block(line, rect(x + 8, y + 14 + 17 * index as i32, 60, 15)))
            .collect(),
    }
}

/// A card's six other texts, two centred above its name box and four below it.
fn other_blocks(card: (i32, i32)) -> Vec<OcrProviderTextBlock> {
    let (x, y) = card;
    [
        ("COVER", rect(x + 16, y + 1, 30, 9)),
        ("NEW", rect(x + 50, y + 1, 40, 9)),
        ("120", rect(x + 4, y + 50, 40, 9)),
        ("LIMIT 1", rect(x + 50, y + 50, 40, 9)),
        ("BUY", rect(x + 4, y + 60, 40, 9)),
        ("STOCK 9", rect(x + 50, y + 60, 40, 9)),
    ]
    .into_iter()
    .map(|(text, at)| block(text, at))
    .collect()
}

/// Test 1's names, card by card: two lines on an even card, one on an odd card.
fn list_names() -> Vec<(&'static str, Vec<&'static str>)> {
    vec![
        ("card-00", vec!["ALPHA", "BRAVO"]),
        ("card-01", vec!["CHARLIE"]),
        ("card-02", vec!["DELTA", "ECHO"]),
        ("card-03", vec!["FOXTROT"]),
        ("card-04", vec!["GOLF", "HOTEL"]),
        ("card-05", vec!["INDIA"]),
        ("card-06", vec!["JULIETT", "KILO"]),
        ("card-07", vec!["LIMA"]),
        ("card-08", vec!["MIKE", "NOVEMBER"]),
        ("card-09", vec!["OSCAR"]),
    ]
}

/// Everything test 1's detector finds: each card's name and its six other texts, 15 name blocks
/// and 60 others.
fn list_blocks() -> Vec<OcrProviderTextBlock> {
    LIST.iter()
        .zip(list_names())
        .flat_map(|(card, (_, lines))| {
            let mut blocks = name_blocks(*card, &lines);
            blocks.extend(other_blocks(*card));
            blocks
        })
        .collect()
}

// A 12 x 10 icon of deterministic gray noise in 1..=255: a shifted copy correlates with it near
// zero.
fn icon() -> Vec<u8> {
    let mut state = 0x0463_0961_0001_u64;
    (0..ICON_WIDTH * ICON_HEIGHT)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            1 + ((state >> 33) % 255) as u8
        })
        .collect()
}

// A frame of `width` x `height`: gray 30, the icon at each card's corner.
fn scene(width: u32, height: u32, cards: &[(i32, i32)]) -> Scene {
    let icon = icon();
    let mut pixels = vec![30_u8; (width * height * 3) as usize];
    for (x, y) in cards {
        for row in 0..ICON_HEIGHT {
            for column in 0..ICON_WIDTH {
                let value = icon[(row * ICON_WIDTH + column) as usize];
                let at = (((*y as u32 + row) * width + *x as u32 + column) * 3) as usize;
                pixels[at..at + 3].copy_from_slice(&[value; 3]);
            }
        }
    }
    Scene::from_rgb8(width, height, &pixels).expect("the frame")
}

// The `member` identity of a card over `names`, `(id, alias)` each, read from its name box.
fn member(names: &[(&str, String)]) -> Value {
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

// The cards' pack on a frame of `width` x `height`: the icon anchors a `repeated_anchor` layout
// whose one feature reads each card's name, with the readable band `band` when given and the
// shared read over `region`.
fn cards_pack(
    width: u32,
    height: u32,
    band: Option<PackRect>,
    region: Value,
    names: &[(&str, String)],
) -> Value {
    let mut layout = json!({
        "id": LAYOUT, "page_id": "neutral/cards", "kind": "repeated_anchor", "anchor": ANCHOR,
        "max_instances": 16, "order": "top_to_bottom", "suppress_iou_milli": 0,
        "instance_rect": {"x": 0, "y": 0, "width": 96, "height": 70},
        "click": {"x": 0, "y": 50, "width": 96, "height": 20},
        "features": [member(names)],
        "shared_ocr": {
            "region": region, "languages": ["en"], "timeout_ms": 5000,
            "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256,
            "blocks": "detect", "line_merge": {"gap_factor": 0}
        }
    });
    if let Some(band) = band {
        layout["readable_band"] = rect_json(band);
    }
    json!({
        "schema_version": "0.7",
        "coordinate_space": {"width": width, "height": height},
        "defaults": {"match_metric": "ccoeff_normed"},
        "targets": [
            {"type": "template", "id": ANCHOR, "template_path": ICON, "region": "full_frame",
             "threshold": 0.9},
            {"type": "ocr", "id": NAME, "region": {"x": 0, "y": 0, "width": 92, "height": 36},
             "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact",
             "expected": ["unused"], "case_sensitive": false, "minimum_confidence": 0.5,
             "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256}
        ],
        "candidate_layouts": [layout]
    })
}

// Test 1's pack: the shared read over the whole frame, which the layout's band is when it
// declares none.
fn list_pack() -> Value {
    let names = list_names()
        .into_iter()
        .map(|(id, lines)| (id, lines.join(" ")))
        .collect::<Vec<_>>();
    cards_pack(LIST_WIDTH, LIST_HEIGHT, None, json!("full_frame"), &names)
}

fn evaluator(pack: Value, provider: Arc<Provider>) -> RecognitionPackResult<RecognitionEvaluator> {
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

// The projection of the cards on `scene`, and the shared read's evidence each name evaluation
// carries, in card order.
fn project(
    evaluator: &RecognitionEvaluator,
    scene: &Scene,
) -> (CandidateProjection, Vec<OcrSharedReadEvidence>) {
    let mut reads = Vec::new();
    let projection = evaluator
        .scene_context(scene)
        .project_candidates_with_samples(LAYOUT, &[scene], &mut |target, result, _, _| {
            if target == NAME
                && let Ok(evaluation) = result
            {
                reads.push(
                    evaluation
                        .ocr
                        .as_ref()
                        .and_then(|ocr| ocr.shared_read)
                        .expect("the name's shared read evidence"),
                );
            }
            Ok(())
        })
        .expect("the cards project");
    (projection, reads)
}

// The `member` identity of each actionable card, `None` where it is Unknown.
fn members(projection: &CandidateProjection) -> Vec<Option<String>> {
    projection
        .candidates()
        .iter()
        .filter(|candidate| candidate.actionable)
        .map(|candidate| match candidate.features.get("member") {
            Some(CandidateFeature::Identity { value, .. }) => Some(value.clone()),
            Some(CandidateFeature::Unknown { .. }) => None,
            other => panic!("{} has the member {other:?}", candidate.id),
        })
        .collect()
}

fn first_frame() -> CandidateSampleVariant {
    CandidateSampleVariant {
        frame: 0,
        dx: 0,
        dy: 0,
        template_metric: None,
    }
}

// Test 1: ten cards in four columns show 75 texts, of which 15 lie in name boxes. Sent no name box,
// as every shared read was before this slice, the read has 75 blocks to recognize, over the cap of
// 64, and the frame fails `text_read_blocks_truncated`; sent the ten name boxes, it recognizes the
// 15 name blocks only and every card reads its name.
#[test]
fn g1_t1_ten_cards_among_seventy_five_texts_read_every_name_from_their_boxes() {
    let blocks = list_blocks();
    assert_eq!(blocks.len(), 75, "setup: 15 name blocks and 60 others");
    let provider = Provider::new(blocks, MAX_BOXES);
    let evaluator = evaluator(list_pack(), provider.clone())
        .expect("ten cards with a shared read over the frame are admitted");
    let scene = scene(LIST_WIDTH, LIST_HEIGHT, &LIST);
    let frame = rect(0, 0, LIST_WIDTH as i32, LIST_HEIGHT as i32);

    // The read as it was made before this slice: no name box.
    let shared = evaluator.pack().candidate_layouts[0]
        .shared_ocr
        .clone()
        .expect("the shared read");
    let first = name_box(LIST[0]);
    let before = evaluator
        .scene_context(&scene)
        .shared_text(
            &shared,
            NAME,
            Some(PackPoint {
                x: first.x,
                y: first.y,
            }),
            first_frame(),
            &mut BTreeMap::new(),
        )
        .expect_err("75 blocks over the cap of 64");
    let outcome = before.outcome().expect("the read's outcome");
    assert!(outcome.is(RecognitionPackCode::TextReadBlocksTruncated));
    assert_eq!(outcome.value("stage"), Some(&json!("text_read_shared")));
    assert_eq!(outcome.value("count"), Some(&json!(75)));
    assert_eq!(outcome.value("limit_count"), Some(&json!(64)));
    assert_eq!(outcome.value("requested"), None);
    assert_eq!(
        provider.seen(),
        [Seen {
            frame: (LIST_WIDTH, LIST_HEIGHT),
            region: frame,
            blocks: DETECT,
            served: Vec::new(),
        }]
    );

    // This slice's read: the ten name boxes, every name read.
    let (projection, reads) = project(&evaluator, &scene);

    let seen = provider.seen();
    assert_eq!(seen.len(), 2, "one request for the projection");
    assert_eq!(
        seen[1],
        Seen {
            frame: (LIST_WIDTH, LIST_HEIGHT),
            region: frame,
            blocks: DETECT,
            served: LIST.iter().map(|card| name_box(*card)).collect(),
        }
    );
    assert_eq!(projection.candidates().len(), LIST.len());
    assert!(
        projection
            .candidates()
            .iter()
            .all(|candidate| candidate.actionable)
    );
    assert_eq!(
        members(&projection),
        list_names()
            .into_iter()
            .map(|(id, _)| Some(id.to_owned()))
            .collect::<Vec<_>>()
    );
    assert_eq!(reads.len(), LIST.len());
    for read in reads {
        assert_eq!(
            read,
            OcrSharedReadEvidence {
                rect: frame,
                detected: Some(75),
                limit: Some(64),
                served: Some(15),
                kept_milli: None,
                threshold_value: None,
            }
        );
    }
}

// Test 3: when the served blocks themselves are more than the cap (15 over a cap of 10), the read
// still fails `text_read_blocks_truncated`, its one cause, and the outcome names the served count
// (`requested`) beside the detected one.
#[test]
fn g1_t3_served_blocks_over_the_cap_still_fail_and_the_outcome_names_their_count() {
    let provider = Provider::new(list_blocks(), 10);
    let evaluator = evaluator(list_pack(), provider.clone())
        .expect("ten cards with a shared read over the frame are admitted");
    let scene = scene(LIST_WIDTH, LIST_HEIGHT, &LIST);

    let failure = evaluator
        .scene_context(&scene)
        .project_candidates(LAYOUT)
        .expect_err("15 served blocks over a cap of 10");

    assert_eq!(failure.code().as_str(), "candidate_feature_failed");
    let outcome = failure
        .cause()
        .and_then(RecognitionPackError::outcome)
        .expect("the read's outcome");
    assert!(outcome.is(RecognitionPackCode::TextReadBlocksTruncated));
    assert_eq!(outcome.value("stage"), Some(&json!("text_read_shared")));
    assert_eq!(outcome.value("count"), Some(&json!(75)));
    assert_eq!(outcome.value("requested"), Some(&json!(15)));
    assert_eq!(outcome.value("limit_count"), Some(&json!(10)));
    let seen = provider.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].served,
        LIST.iter().map(|card| name_box(*card)).collect::<Vec<_>>()
    );
}

// Test 5's located text: the region of a list, and `total` texts in it: the name it looks for
// first when `named`, then filler texts in a grid.
const LOCATE_WIDTH: u32 = 640;
const LOCATE_HEIGHT: u32 = 360;
const LOCATE_REGION: PackRect = PackRect {
    x: 40,
    y: 60,
    width: 560,
    height: 280,
};

fn locate_blocks(named: bool, total: usize) -> Vec<OcrProviderTextBlock> {
    let mut blocks = Vec::new();
    if named {
        blocks.push(block("ALPHA", rect(44, 300, 60, 20)));
    }
    for index in 0..total - usize::from(named) {
        let (column, row) = ((index % 13) as i32, (index / 13) as i32);
        blocks.push(block("FILLER", rect(44 + 40 * column, 64 + 40 * row, 8, 6)));
    }
    blocks
}

fn locate_pack() -> Value {
    let target = |id: &str, locate: Option<Value>| {
        let mut target = json!({
            "type": "ocr", "id": id, "region": rect_json(LOCATE_REGION), "languages": ["en"],
            "timeout_ms": 1000, "match_mode": "exact", "expected": ["ALPHA"],
            "case_sensitive": false, "minimum_confidence": 0.8, "model_ref": "PP-OCRv6_medium",
            "model_sha256": MODEL_SHA256
        });
        if let Some(locate) = locate {
            target["locate"] = locate;
        }
        target
    };
    json!({
        "schema_version": "0.7",
        "coordinate_space": {"width": LOCATE_WIDTH, "height": LOCATE_HEIGHT},
        "targets": [
            target("text/entry", Some(json!({
                "normalize": "identity_v1", "blocks": "detect", "line_merge": {"gap_factor": 0}
            }))),
            target("text/plain", None)
        ]
    })
}

fn locate_evaluator(provider: Arc<Provider>) -> RecognitionEvaluator {
    RecognitionEvaluator::with_vision_provider(
        load_pack_from_json_str(&locate_pack().to_string()).expect("the located text's pack"),
        Arc::new(FsAssetResolver::new(PathBuf::new())),
        provider,
    )
    .expect("the located text's evaluator")
}

fn locate_scene() -> Scene {
    Scene::from_rgb8(
        LOCATE_WIDTH,
        LOCATE_HEIGHT,
        &vec![0; (LOCATE_WIDTH * LOCATE_HEIGHT * 3) as usize],
    )
    .expect("blank scene")
}

// Test 5: a located text, and an OCR target read on its own, send no name box, and a located text
// answers as before at the cap and over it: at 64 detected blocks its hit passes untruncated and a
// miss is `text_locate_not_found`; at 65 its hit passes truncated and a miss fails
// `text_read_blocks_truncated` with the detected count and no `requested`.
#[test]
fn g1_t5_a_located_text_sends_no_name_box_and_answers_as_before_at_and_over_the_cap() {
    let scene = locate_scene();
    let read = |named: bool, blocks: usize| {
        let provider = Provider::new(locate_blocks(named, blocks), MAX_BOXES);
        let evaluated = locate_evaluator(provider.clone()).evaluate_target(&scene, "text/entry");
        let seen = provider.seen();
        assert_eq!(seen.len(), 1, "named {named}, {blocks} blocks");
        assert_eq!(seen[0].region, LOCATE_REGION);
        assert_eq!(seen[0].blocks, DETECT);
        assert!(seen[0].served.is_empty(), "named {named}, {blocks} blocks");
        evaluated
    };
    let located = |evaluation: &TargetEvaluation| {
        evaluation
            .ocr
            .as_ref()
            .and_then(|ocr| ocr.located)
            .expect("located evidence")
    };

    let at_cap = read(true, 64).expect("a hit at the cap");
    assert!(at_cap.passed);
    assert!(!located(&at_cap).truncated);
    let missed_at_cap = read(false, 64).expect("a miss at the cap concludes");
    assert!(!missed_at_cap.passed);
    assert_eq!(
        missed_at_cap
            .failure
            .as_ref()
            .expect("a typed failure")
            .code()
            .as_str(),
        "text_locate_not_found"
    );

    let over_cap = read(true, 65).expect("a hit among 65 blocks");
    assert!(over_cap.passed);
    assert!(located(&over_cap).truncated);
    let error = read(false, 65).expect_err("a miss among 65 blocks cannot conclude");
    let outcome = error.outcome().expect("the read's outcome");
    assert!(outcome.is(RecognitionPackCode::TextReadBlocksTruncated));
    assert_eq!(outcome.value("stage"), Some(&json!("text_locate_evaluate")));
    assert_eq!(outcome.value("count"), Some(&json!(65)));
    assert_eq!(outcome.value("limit_count"), Some(&json!(64)));
    assert_eq!(outcome.value("requested"), None);

    // An OCR target without `locate`, read on its own, sends no name box either.
    let provider = Provider::new(vec![block("ALPHA", rect(44, 300, 60, 20))], MAX_BOXES);
    let plain = locate_evaluator(provider.clone())
        .evaluate_target(&scene, "text/plain")
        .expect("the plain read");
    assert!(plain.passed);
    let seen = provider.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].region, LOCATE_REGION);
    assert_eq!(seen[0].blocks, OcrProviderBlocks::Implicit);
    assert!(seen[0].served.is_empty());
}

// Test 8's cards: four rows of four; each card's one-line name, and for the two cut rows a name
// line inside the band where its own name box would be.
fn grid_cards() -> Vec<(i32, i32)> {
    [10, 90, 170, 250]
        .into_iter()
        .flat_map(|y| [10, 110, 210, 310].into_iter().map(move |x| (x, y)))
        .collect()
}

fn grid_blocks() -> Vec<OcrProviderTextBlock> {
    grid_cards()
        .into_iter()
        .enumerate()
        .map(|(index, (x, y))| {
            let text = format!("NAME{index:02}");
            match y {
                // The band starts at 40: the first row's line lies below that, in its name box.
                10 => block(&text, rect(x + 10, 41, 70, 6)),
                // The band ends at 290: the last row's line lies above that, in its name box.
                250 => block(&text, rect(x + 10, 266, 70, 16)),
                _ => block(&text, rect(x + 10, y + 22, 70, 16)),
            }
        })
        .collect()
}

// Test 8: of sixteen anchors only the eight cards the band holds whole are actionable; the read is
// sent their eight name boxes and no box of a card the band cuts, recognizes their eight names and
// not the cut cards' lines, and every whole card reads its name.
#[test]
fn g1_t8_cards_the_band_cuts_send_no_name_box() {
    let cards = grid_cards();
    let whole = cards[4..12].to_vec();
    let names = (4..12)
        .map(|index| (format!("name-{index:02}"), format!("NAME{index:02}")))
        .collect::<Vec<_>>();
    let entries = names
        .iter()
        .map(|(id, alias)| (id.as_str(), alias.clone()))
        .collect::<Vec<_>>();
    let provider = Provider::new(grid_blocks(), MAX_BOXES);
    let evaluator = evaluator(
        cards_pack(
            GRID_WIDTH,
            GRID_HEIGHT,
            Some(GRID_BAND),
            rect_json(GRID_BAND),
            &entries,
        ),
        provider.clone(),
    )
    .expect("a band that cuts two rows is admitted");
    let scene = scene(GRID_WIDTH, GRID_HEIGHT, &cards);

    let (projection, reads) = project(&evaluator, &scene);

    assert_eq!(projection.candidates().len(), cards.len());
    for (index, candidate) in projection.candidates().iter().enumerate() {
        assert_eq!(
            candidate.actionable,
            (4..12).contains(&index),
            "{candidate:?}"
        );
        if !candidate.actionable {
            assert!(candidate.features.is_empty(), "{candidate:?}");
        }
    }
    let seen = provider.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].region, GRID_BAND);
    assert_eq!(
        seen[0].served,
        whole.iter().map(|card| name_box(*card)).collect::<Vec<_>>()
    );
    assert_eq!(
        members(&projection),
        names
            .iter()
            .map(|(id, _)| Some(id.clone()))
            .collect::<Vec<_>>()
    );
    assert_eq!(reads.len(), whole.len());
    for read in reads {
        assert_eq!(read.detected, Some(16));
        assert_eq!(read.served, Some(8));
    }
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
