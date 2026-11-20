// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 SEL-GRID G2 (register §1.491; contracts/selection-graph.md, "Shared OCR read",
// "The lines of a name box"; the coordinator's work order rd-1119/brief-463-g2-name-lines.md, the
// kernel's side of its tests): a layout's shared read may declare the lines of its name boxes; the
// read then sends them with the name boxes it serves to the provider's read with lines, gives each
// cut block its strips, which make its text by the one join of a name box, and keeps the served
// blocks below the floor out of every name. Admission bounds the lines and refuses them on a read
// that serves a target placed only on the frame.
//
// Every expected value comes from those rules and the fixtures below, never from the code. The
// frame is flat gray; a `fixed_slots` layout of two slots reads each slot's name through one shared
// read over the frame, slot 0's name box (10, 10, 80, 60), slot 1's (110, 10, 80, 60). Slot 0
// shows a name of two lines, ALPHA at (20, 20, 60, 21) and BRAVO at (20, 44, 60, 21), 3 px apart,
// which the detector may find apart or as one block (20, 20, 60, 45); slot 1 a name of one line,
// CHARLIE at (120, 30, 60, 22). A scripted provider answers a read without lines with the blocks
// it is given (two lines found as one block it reads as a recognizer of one line reads them, poorly,
// at 0.3) and a read with lines with the blocks, strips and blocks below the floor it is given, as
// the text provider's contract says it cuts; it records every request. The gate reads no clock.

use crate::*;
use actingcommand_contract::CandidateFeature;
use actingcommand_contract::candidate_projection::{
    CandidateIdentitySource, CandidateProjection, CandidateUnknownReason,
};
use serde_json::{Value, json};
use std::sync::Mutex;

const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const LAYOUT: &str = "layout/slots";
const WIDTH: u32 = 200;
const HEIGHT: u32 = 120;
const NAME_0: PackRect = PackRect {
    x: 10,
    y: 10,
    width: 80,
    height: 60,
};
const NAME_1: PackRect = PackRect {
    x: 110,
    y: 10,
    width: 80,
    height: 60,
};
const ALPHA: PackRect = PackRect {
    x: 20,
    y: 20,
    width: 60,
    height: 21,
};
const BRAVO: PackRect = PackRect {
    x: 20,
    y: 44,
    width: 60,
    height: 21,
};
const MERGED: PackRect = PackRect {
    x: 20,
    y: 20,
    width: 60,
    height: 45,
};
const CHARLIE: PackRect = PackRect {
    x: 120,
    y: 30,
    width: 60,
    height: 22,
};
/// The lines the read declares: two of 21 px.
const LINES: OcrProviderLines = OcrProviderLines {
    max: 2,
    line_height: 21,
};

/// What one request asked: its name boxes, and its lines when it was a read with lines.
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    served: Vec<PackRect>,
    lines: Option<OcrProviderLines>,
}

/// The answer of a read with lines: each block with its strips, the blocks below the floor, and
/// the count's served recognitions when the test gives them (else one a block, a cut block its
/// strips).
#[derive(Default)]
struct Cut {
    blocks: Vec<(OcrProviderTextBlock, Vec<OcrProviderTextBlock>)>,
    below_floor: Vec<PackRect>,
    count: Option<OcrProviderDetection>,
}

/// The scripted provider of the module comment.
struct Provider {
    plain: Vec<OcrProviderTextBlock>,
    cut: Cut,
    seen: Mutex<Vec<Seen>>,
}

impl fmt::Debug for Provider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Provider")
    }
}

impl Provider {
    fn new(plain: Vec<OcrProviderTextBlock>, cut: Cut) -> Arc<Self> {
        Arc::new(Self {
            plain,
            cut,
            seen: Mutex::new(Vec::new()),
        })
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().expect("request log").clone()
    }
}

fn count(blocks: usize) -> u32 {
    u32::try_from(blocks).expect("a count")
}

fn answer(blocks: Vec<OcrProviderTextBlock>, detection: OcrProviderDetection) -> OcrProviderResult {
    OcrProviderResult {
        ppocr_diagnostics: Vec::new(),
        text: blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        blocks,
        confidence: Some(0.9),
        detection: Some(detection),
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
            served: request.served.to_vec(),
            lines: None,
        });
        let detection = OcrProviderDetection {
            detected: count(self.plain.len()),
            limit: 64,
            served: (!request.served.is_empty()).then(|| count(self.plain.len())),
        };
        Ok(answer(self.plain.clone(), detection))
    }

    fn read_text_lines(
        &self,
        request: OcrProviderRequest<'_>,
        lines: OcrProviderLines,
    ) -> Result<OcrProviderLinesResult, VisionProviderError> {
        self.seen.lock().expect("request log").push(Seen {
            served: request.served.to_vec(),
            lines: Some(lines),
        });
        let recognitions = self
            .cut
            .blocks
            .iter()
            .map(|(_, strips)| count(strips.len().max(1)))
            .sum::<u32>();
        let detection = self.cut.count.unwrap_or(OcrProviderDetection {
            detected: count(self.cut.blocks.len() + self.cut.below_floor.len()),
            limit: 64,
            served: Some(recognitions),
        });
        Ok(OcrProviderLinesResult {
            result: answer(
                self.cut
                    .blocks
                    .iter()
                    .map(|(block, _)| block.clone())
                    .collect(),
                detection,
            ),
            strips: self
                .cut
                .blocks
                .iter()
                .map(|(_, strips)| strips.clone())
                .collect(),
            below_floor: self.cut.below_floor.clone(),
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

fn block(text: &str, rect: PackRect, confidence: f32) -> OcrProviderTextBlock {
    OcrProviderTextBlock {
        text: text.to_owned(),
        rect,
        confidence: Some(confidence),
    }
}

/// Slot 0's two lines as a cut block: "ALPHA\nBRAVO" at the lowest of its strips' 0.95.
fn cut_name() -> (OcrProviderTextBlock, Vec<OcrProviderTextBlock>) {
    (
        block("ALPHA\nBRAVO", MERGED, 0.95),
        vec![block("ALPHA", ALPHA, 0.95), block("BRAVO", BRAVO, 0.95)],
    )
}

/// Slot 1's one line, read whole.
fn whole_charlie() -> (OcrProviderTextBlock, Vec<OcrProviderTextBlock>) {
    (block("CHARLIE", CHARLIE, 0.95), Vec::new())
}

fn rect_json(rect: PackRect) -> Value {
    json!({"x": rect.x, "y": rect.y, "width": rect.width, "height": rect.height})
}

/// The slots' pack, its shared read declaring `lines` when given (`Value::Null`: none).
fn slots_pack(lines: Value) -> Value {
    let target = |id: &str, region: PackRect| {
        json!({
            "type": "ocr", "id": id, "region": rect_json(region), "languages": ["en"],
            "timeout_ms": 1000, "match_mode": "exact", "expected": ["unused"],
            "case_sensitive": false, "minimum_confidence": 0.5,
            "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256
        })
    };
    let mut shared = json!({
        "region": rect_json(PackRect { x: 0, y: 0, width: 200, height: 120 }),
        "languages": ["en"], "timeout_ms": 5000, "model_ref": "PP-OCRv6_medium",
        "model_sha256": MODEL_SHA256, "blocks": "detect", "line_merge": {"gap_factor": 0}
    });
    if !lines.is_null() {
        shared["lines"] = lines;
    }
    let slot = |x: i32, target: &str| {
        let rect = rect_json(PackRect {
            x,
            y: 5,
            width: 90,
            height: 110,
        });
        json!({"rect": rect, "click": rect, "targets": {"member": target}})
    };
    json!({
        "schema_version": "0.7",
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "targets": [target("slot/name_00", NAME_0), target("slot/name_01", NAME_1)],
        "candidate_layouts": [{
            "id": LAYOUT, "page_id": "neutral/slots", "kind": "fixed_slots",
            "features": [{
                "name": "member", "value": "identity",
                "identity": {"entries": [
                    {"id": "alpha_bravo", "aliases": ["ALPHA BRAVO"]},
                    {"id": "charlie", "aliases": ["CHARLIE"]}
                ], "recognition": {
                    "kind": "ocr_aliases", "max_distance": 0, "minimum_margin": 1,
                    "minimum_confidence_milli": 800
                }}
            }],
            "slots": [slot(5, "slot/name_00"), slot(105, "slot/name_01")],
            "shared_ocr": shared
        }]
    })
}

fn lines_json() -> Value {
    json!({"max": 2, "line_height": 21})
}

fn evaluator(pack: Value, provider: Arc<Provider>) -> RecognitionPackResult<RecognitionEvaluator> {
    RecognitionEvaluator::with_vision_provider(
        load_pack_from_json_str(&pack.to_string())?,
        Arc::new(FsAssetResolver::new(PathBuf::new())),
        provider,
    )
}

fn scene() -> Scene {
    Scene::from_rgb8(WIDTH, HEIGHT, &vec![60; (WIDTH * HEIGHT * 3) as usize]).expect("the frame")
}

/// The slots' projection on the frame, and each slot's name evaluation in slot order.
fn project(evaluator: &RecognitionEvaluator) -> (CandidateProjection, Vec<TargetEvaluation>) {
    let scene = scene();
    let mut names = Vec::new();
    let projection = evaluator
        .scene_context(&scene)
        .project_candidates_with_samples(LAYOUT, &[&scene], &mut |target, result, _, _| {
            if target.starts_with("slot/name_")
                && let Ok(evaluation) = result
            {
                names.push(evaluation.clone());
            }
            Ok(())
        })
        .expect("the slots project");
    (projection, names)
}

/// Each slot's `member`: its identity, or the reason it is Unknown.
fn members(projection: &CandidateProjection) -> Vec<Result<String, CandidateUnknownReason>> {
    projection
        .candidates()
        .iter()
        .map(|candidate| match candidate.features.get("member") {
            Some(CandidateFeature::Identity { value, .. }) => Ok(value.clone()),
            Some(CandidateFeature::Unknown { reason, .. }) => Err(*reason),
            other => panic!("{} has the member {other:?}", candidate.id),
        })
        .collect()
}

fn ocr(evaluation: &TargetEvaluation) -> &OcrEvaluation {
    evaluation.ocr.as_deref().expect("the name's OCR evidence")
}

// Test 1, the kernel's side: slot 0's two lines found as one block. Read as today, without the
// member, the read sends no lines and the provider reads the block as one line, poorly (0.3):
// slot 0's name is Unknown, low_confidence. With `lines`, the read sends {max 2, line_height 21}
// with both name boxes, the provider cuts the block and slot 0 reads "ALPHA\nBRAVO" at 0.95, its
// identity alpha_bravo, its block carrying the two strips; found apart, as two blocks read without
// lines, it reads the same text at the same confidence and the same identity.
#[test]
fn g2_two_lines_read_through_a_cut_are_the_name_read_when_detected_apart() {
    let boxes = vec![NAME_0, NAME_1];

    let today = Provider::new(
        vec![
            block("ALPHABRAVO", MERGED, 0.3),
            block("CHARLIE", CHARLIE, 0.95),
        ],
        Cut::default(),
    );
    let (projection, _) =
        project(&evaluator(slots_pack(Value::Null), today.clone()).expect("admitted"));
    assert_eq!(
        members(&projection),
        [
            Err(CandidateUnknownReason::LowConfidence),
            Ok("charlie".to_owned())
        ]
    );
    assert_eq!(
        today.seen(),
        [Seen {
            served: boxes.clone(),
            lines: None
        }]
    );

    let cutting = Provider::new(
        Vec::new(),
        Cut {
            blocks: vec![cut_name(), whole_charlie()],
            ..Cut::default()
        },
    );
    let (cut_projection, cut_names) =
        project(&evaluator(slots_pack(lines_json()), cutting.clone()).expect("admitted"));
    assert_eq!(
        cutting.seen(),
        [Seen {
            served: boxes,
            lines: Some(LINES)
        }]
    );
    assert_eq!(
        members(&cut_projection),
        [Ok("alpha_bravo".to_owned()), Ok("charlie".to_owned())]
    );
    let cut = ocr(&cut_names[0]);
    assert_eq!(cut.text, "ALPHA\nBRAVO");
    assert_eq!(cut.confidence, Some(0.95));
    assert_eq!(
        cut.blocks,
        [OcrTextEvidence {
            text: "ALPHA\nBRAVO".to_owned(),
            rect: MERGED,
            confidence: Some(0.95),
            strips: vec![
                OcrStripEvidence {
                    text: "ALPHA".to_owned(),
                    rect: ALPHA,
                    confidence: Some(0.95),
                },
                OcrStripEvidence {
                    text: "BRAVO".to_owned(),
                    rect: BRAVO,
                    confidence: Some(0.95),
                },
            ],
        }]
    );
    assert_eq!(
        cut.shared_read.and_then(|read| read.served),
        Some(3),
        "a cut block counts as its two strips"
    );

    let apart = Provider::new(
        vec![
            block("ALPHA", ALPHA, 0.95),
            block("BRAVO", BRAVO, 0.95),
            block("CHARLIE", CHARLIE, 0.95),
        ],
        Cut::default(),
    );
    let (apart_projection, apart_names) =
        project(&evaluator(slots_pack(Value::Null), apart).expect("admitted"));
    assert_eq!(members(&apart_projection), members(&cut_projection));
    let detected_apart = ocr(&apart_names[0]);
    assert_eq!(detected_apart.text, cut.text);
    assert_eq!(detected_apart.confidence, cut.confidence);
    assert_eq!(detected_apart.blocks.len(), 2);
    assert!(
        detected_apart
            .blocks
            .iter()
            .all(|block| block.strips.is_empty())
    );
}

// Test 3, the kernel's side: names of one line in boxes that allow two. The provider reads each
// block whole, with or without lines, and answers alike; each name's evidence is then the same,
// byte for byte, with or without the member.
#[test]
fn g2_a_name_of_one_line_reads_byte_for_byte_as_without_lines() {
    let one_line = || {
        vec![
            block("CHARLIE", PackRect { x: 20, ..CHARLIE }, 0.95),
            block("CHARLIE", CHARLIE, 0.95),
        ]
    };
    let without = Provider::new(one_line(), Cut::default());
    let with = Provider::new(
        Vec::new(),
        Cut {
            blocks: one_line()
                .into_iter()
                .map(|block| (block, Vec::new()))
                .collect(),
            ..Cut::default()
        },
    );

    let (_, plain) = project(&evaluator(slots_pack(Value::Null), without).expect("admitted"));
    let (_, lined) = project(&evaluator(slots_pack(lines_json()), with).expect("admitted"));

    assert_eq!(plain.len(), 2);
    for (plain, lined) in plain.iter().zip(&lined) {
        assert_eq!(
            serde_json::to_string(&plain.ocr).expect("evidence"),
            serde_json::to_string(&lined.ocr).expect("evidence")
        );
    }
}

// Test 5, the kernel's side: with lines, a served block lower than half a line inside slot 0's name
// box (60, 64, 20, 4) is answered below the floor: it is in no text, slot 0 still reads its name,
// and slot 0's evidence counts it in `below_floor`; slot 1's holds none.
#[test]
fn g2_a_block_below_the_floor_is_in_no_name_and_is_counted() {
    let speck = PackRect {
        x: 60,
        y: 64,
        width: 20,
        height: 4,
    };
    let provider = Provider::new(
        Vec::new(),
        Cut {
            blocks: vec![cut_name(), whole_charlie()],
            below_floor: vec![speck],
            count: None,
        },
    );

    let (projection, names) =
        project(&evaluator(slots_pack(lines_json()), provider).expect("admitted"));

    assert_eq!(
        members(&projection),
        [Ok("alpha_bravo".to_owned()), Ok("charlie".to_owned())]
    );
    assert_eq!(ocr(&names[0]).text, "ALPHA\nBRAVO");
    assert_eq!(ocr(&names[0]).below_floor, [speck]);
    assert!(ocr(&names[1]).below_floor.is_empty());
}

// Edge test, a colour chain: the read over (5, 5, 190, 110) and both names declare the same chain
// (keep near-white, invert), so the provider is given the processed region alone, its origin at
// the read's (5, 5), the name boxes moved onto it, and answers its blocks, strips and blocks below
// the floor in that picture. The read moves each back onto the region's origin: slot 0's evidence
// holds the cut block, its two strips and the speck of test 5 in the frame's coordinates, where
// the read without a chain holds them (tests 1 and 5), and the names read as there.
#[test]
fn g2_under_a_colour_chain_the_strips_are_in_the_frame_coordinates() {
    const SHARED: PackRect = PackRect {
        x: 5,
        y: 5,
        width: 190,
        height: 110,
    };
    let in_picture = |rect: PackRect| PackRect {
        x: rect.x - SHARED.x,
        y: rect.y - SHARED.y,
        ..rect
    };
    let chain = json!({"steps": [
        {"kind": "keep_colors", "colors": [[255, 255, 255], [240, 240, 232]], "tolerance": 40},
        {"kind": "invert"}
    ]});
    let mut pack = slots_pack(lines_json());
    pack["candidate_layouts"][0]["shared_ocr"]["region"] = rect_json(SHARED);
    pack["candidate_layouts"][0]["shared_ocr"]["preprocess"] = chain.clone();
    for target in [0_usize, 1] {
        pack["targets"][target]["preprocess"] = chain.clone();
    }
    let speck = PackRect {
        x: 60,
        y: 64,
        width: 20,
        height: 4,
    };
    let provider = Provider::new(
        Vec::new(),
        Cut {
            blocks: vec![
                (
                    block("ALPHA\nBRAVO", in_picture(MERGED), 0.95),
                    vec![
                        block("ALPHA", in_picture(ALPHA), 0.95),
                        block("BRAVO", in_picture(BRAVO), 0.95),
                    ],
                ),
                (block("CHARLIE", in_picture(CHARLIE), 0.95), Vec::new()),
            ],
            below_floor: vec![in_picture(speck)],
            count: None,
        },
    );

    let (projection, names) = project(&evaluator(pack, provider.clone()).expect("admitted"));

    assert_eq!(
        provider.seen(),
        [Seen {
            served: vec![in_picture(NAME_0), in_picture(NAME_1)],
            lines: Some(LINES)
        }]
    );
    assert_eq!(
        members(&projection),
        [Ok("alpha_bravo".to_owned()), Ok("charlie".to_owned())]
    );
    let name = ocr(&names[0]);
    assert!(name.preprocess.is_some(), "the chain's evidence");
    assert_eq!(name.text, "ALPHA\nBRAVO");
    assert_eq!(
        name.blocks
            .iter()
            .map(|block| block.rect)
            .collect::<Vec<_>>(),
        [MERGED]
    );
    assert_eq!(
        name.blocks[0]
            .strips
            .iter()
            .map(|strip| strip.rect)
            .collect::<Vec<_>>(),
        [ALPHA, BRAVO]
    );
    assert_eq!(name.below_floor, [speck]);
    assert_eq!(
        ocr(&names[1])
            .blocks
            .iter()
            .map(|block| block.rect)
            .collect::<Vec<_>>(),
        [CHARLIE]
    );
}

// Test 6, the kernel's side: served blocks under the cap whose strips exceed it (the provider
// counts 6 recognitions of 3 blocks over a cap of 4) end the frame text_read_blocks_truncated, its
// one cause, with the recognitions as `requested` beside the detected count.
#[test]
fn g2_strips_over_the_cap_end_the_read_truncated() {
    let provider = Provider::new(
        Vec::new(),
        Cut {
            blocks: vec![cut_name(), whole_charlie()],
            below_floor: Vec::new(),
            count: Some(OcrProviderDetection {
                detected: 3,
                limit: 4,
                served: Some(6),
            }),
        },
    );
    let evaluator = evaluator(slots_pack(lines_json()), provider).expect("admitted");

    let failure = evaluator
        .scene_context(&scene())
        .project_candidates(LAYOUT)
        .expect_err("6 recognitions over a cap of 4");

    assert_eq!(failure.code().as_str(), "candidate_feature_failed");
    let outcome = failure
        .cause()
        .and_then(RecognitionPackError::outcome)
        .expect("the read's outcome");
    assert!(outcome.is(RecognitionPackCode::TextReadBlocksTruncated));
    assert_eq!(outcome.value("stage"), Some(&json!("text_read_shared")));
    assert_eq!(outcome.value("count"), Some(&json!(3)));
    assert_eq!(outcome.value("requested"), Some(&json!(6)));
    assert_eq!(outcome.value("limit_count"), Some(&json!(4)));
}

// A read with lines fails loud on a provider that cannot cut (the trait's default refuses), and on
// a cut block whose text is not its strips' joined: the projection fails, candidate_feature_failed
// caused by the provider's refusal or its invalid answer.
#[test]
fn g2_a_provider_that_cannot_cut_or_cuts_badly_fails_the_read() {
    #[derive(Debug)]
    struct Uncut;

    impl VisionProvider for Uncut {
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
            Ok(())
        }

        fn read_text(
            &self,
            _request: OcrProviderRequest<'_>,
        ) -> Result<OcrProviderResult, VisionProviderError> {
            Ok(answer(
                Vec::new(),
                OcrProviderDetection {
                    detected: 0,
                    limit: 64,
                    served: Some(0),
                },
            ))
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

    let evaluator_of = |provider: Arc<dyn VisionProvider>| {
        RecognitionEvaluator::with_vision_provider(
            load_pack_from_json_str(&slots_pack(lines_json()).to_string()).expect("the pack"),
            Arc::new(FsAssetResolver::new(PathBuf::new())),
            provider,
        )
        .expect("admitted")
    };
    let failure = evaluator_of(Arc::new(Uncut))
        .scene_context(&scene())
        .project_candidates(LAYOUT)
        .expect_err("a provider that cannot cut");
    assert_eq!(failure.code().as_str(), "candidate_feature_failed");
    assert_eq!(
        failure.cause().map(RecognitionPackError::code),
        Some(RecognitionPackErrorCode::VisionProviderFailure)
    );
    assert!(
        failure
            .cause()
            .is_some_and(|cause| cause.message().contains("Unavailable")),
        "{failure:?}"
    );

    let (_, strips) = cut_name();
    let badly = Provider::new(
        Vec::new(),
        Cut {
            blocks: vec![(block("ALPHA BRAVO", MERGED, 0.95), strips)],
            ..Cut::default()
        },
    );
    let failure = evaluator_of(badly)
        .scene_context(&scene())
        .project_candidates(LAYOUT)
        .expect_err("a cut block that is not its strips");
    assert_eq!(
        failure.cause().map(RecognitionPackError::code),
        Some(RecognitionPackErrorCode::VisionProviderInvalidResponse)
    );
}

// Test 8: the two cause words. A reading under the confidence threshold and a reading that passes
// it and matches no alias within `max_distance` keep the one Unknown reason of the feature,
// low_confidence (no decision changes), and are told apart by their causes, low_confidence and
// beyond_distance; a reading that matches has none.
#[test]
fn g2_a_reading_beyond_the_distance_has_a_cause_of_its_own() {
    let identity: CandidateIdentityDeclaration = serde_json::from_value(json!({
        "entries": [{"id": "alpha_bravo", "aliases": ["ALPHA BRAVO"]}],
        "recognition": {"kind": "ocr_aliases", "max_distance": 1, "minimum_margin": 1,
                        "minimum_confidence_milli": 800}
    }))
    .expect("an identity");
    let low = CandidateFeature::Unknown {
        reason: CandidateUnknownReason::LowConfidence,
        source: Some(CandidateIdentitySource::OcrAlias),
    };

    let below = identity.map_ocr_with_cause("ALPHA BRAVO", Some(500));
    assert_eq!(
        below,
        (low.clone(), Some(CandidateUnreadCause::LowConfidence))
    );
    assert_eq!(identity.map_ocr("ALPHA BRAVO", Some(500)), low);

    let beyond = identity.map_ocr_with_cause("ZULU", Some(950));
    assert_eq!(
        beyond,
        (low.clone(), Some(CandidateUnreadCause::BeyondDistance))
    );
    assert_eq!(identity.map_ocr("ZULU", Some(950)), low);
    assert_eq!(
        CandidateUnreadCause::BeyondDistance.as_str(),
        "beyond_distance"
    );
    assert_eq!(
        serde_json::to_value(CandidateUnreadCause::BeyondDistance).expect("the word"),
        json!("beyond_distance")
    );

    let (read, cause) = identity.map_ocr_with_cause("ALPHA\nBRAVO", Some(950));
    assert!(
        matches!(read, CandidateFeature::Identity { .. }),
        "{read:?}"
    );
    assert_eq!(cause, None);
}

// Admission: `lines` holds `max` 1 to 3 and `line_height` of at least 8, each refused at its
// pointer otherwise; a member left out or of another type is refused by the wire's shape; and a
// read that declares lines and gives text to a target placed only on the frame (template_relative)
// is refused, since such a target's name box is never sent. The read without the member, and with
// the lines of the tests, is admitted.
#[test]
fn g2_admission_bounds_the_lines_and_needs_boxes_it_can_place() {
    let provider = || Provider::new(Vec::new(), Cut::default());
    for (lines, pointer) in [
        (
            json!({"max": 0, "line_height": 21}),
            "/shared_ocr/lines/max",
        ),
        (
            json!({"max": 4, "line_height": 21}),
            "/shared_ocr/lines/max",
        ),
        (
            json!({"max": 2, "line_height": 7}),
            "/shared_ocr/lines/line_height",
        ),
        (json!({"max": 2}), "/shared_ocr/lines lacks line_height"),
        (
            json!({"max": "2", "line_height": 21}),
            "/shared_ocr/lines/max",
        ),
        (
            json!({"max": 2, "line_height": 21, "gap": 3}),
            "/shared_ocr/lines",
        ),
    ] {
        let refused = evaluator(slots_pack(lines.clone()), provider())
            .expect_err("lines out of their bounds or shape");
        assert!(
            refused.message().contains(pointer),
            "{lines}: {}",
            refused.message()
        );
    }
    evaluator(slots_pack(Value::Null), provider()).expect("without lines");
    evaluator(slots_pack(lines_json()), provider()).expect("with lines");

    let mut placed_on_frame = slots_pack(lines_json());
    placed_on_frame["targets"][1]["region"] = json!({
        "mode": "template_relative", "anchor_target_id": "slot/name_00",
        "offset": {"x": 100, "y": 0}, "width": 80, "height": 60
    });
    let refused = evaluator(placed_on_frame, provider())
        .expect_err("lines on a read that serves a template_relative target");
    assert!(
        refused.message().contains("/shared_ocr/lines")
            && refused.message().contains("slot/name_01"),
        "{}",
        refused.message()
    );
}
