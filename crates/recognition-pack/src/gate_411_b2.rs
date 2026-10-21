// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 B2 (b-plan §2.2 and its BT-B2 test T2; contracts/business-identity-consensus.md;
// register §1.146 B2-2; exec key 116): an `ocr_integer` feature of format `current_capacity`
// reads the part of `a/b` its optional `part` names: `capacity` the right-hand part, `current`
// the left-hand one, and a feature without `part` the left-hand one as before. A scripted OCR
// provider answers the one slot's region with `17/20` on a blank neutral frame.

use crate::*;
use actingcommand_contract::CandidateFeature;
use serde_json::{Value, json};

const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const FRAME_WIDTH: u32 = 640;
const FRAME_HEIGHT: u32 = 360;
const LAYOUT: &str = "layout/counter";
const SLOT: PackRect = PackRect {
    x: 40,
    y: 40,
    width: 120,
    height: 30,
};
const FRACTION: &str = "17/20";

// Answers every OCR request with `FRACTION` as one block over the requested region.
#[derive(Debug)]
struct FractionText;

impl VisionProvider for FractionText {
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
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: FRACTION.to_owned(),
            blocks: vec![OcrProviderTextBlock {
                text: FRACTION.to_owned(),
                rect: request.region,
                confidence: Some(0.9),
            }],
            confidence: Some(0.9),
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

// A pack with one OCR target and a one-slot layout whose `count` feature reads it as an
// `ocr_integer` of format `current_capacity`, with `part` when given.
fn counter_pack(part: Option<&str>) -> Value {
    let region = json!({"x": SLOT.x, "y": SLOT.y, "width": SLOT.width, "height": SLOT.height});
    let mut integer = json!({
        "min": 0, "max": 999, "format": "current_capacity", "minimum_confidence_milli": 800
    });
    if let Some(part) = part {
        integer["part"] = json!(part);
    }
    json!({
        "schema_version": "0.7",
        "coordinate_space": {"width": FRAME_WIDTH, "height": FRAME_HEIGHT},
        "targets": [{
            "type": "ocr", "id": "slot/count", "region": region, "languages": ["en"],
            "timeout_ms": 1000, "match_mode": "exact", "expected": ["unused"],
            "case_sensitive": false, "minimum_confidence": 0.5, "model_ref": "PP-OCRv6_medium",
            "model_sha256": MODEL_SHA256
        }],
        "candidate_layouts": [{
            "id": LAYOUT, "page_id": "neutral/counter", "kind": "fixed_slots",
            "features": [{"name": "count", "value": "ocr_integer", "integer": integer}],
            "slots": [{"rect": region, "click": region, "targets": {"count": "slot/count"}}]
        }]
    })
}

#[test]
fn b2_t2_ocr_integer_with_part_capacity_reads_the_right_hand_part() {
    let scene = Scene::from_rgb8(
        FRAME_WIDTH,
        FRAME_HEIGHT,
        &vec![0; (FRAME_WIDTH * FRAME_HEIGHT * 3) as usize],
    )
    .expect("blank scene");
    // §2.2: `part: capacity` reads the right-hand part of `a/b`; `current`, and a feature
    // without `part`, the left-hand one.
    for (part, expected) in [(Some("capacity"), 20), (Some("current"), 17), (None, 17)] {
        let pack = load_pack_from_json_str(&counter_pack(part).to_string())
            .unwrap_or_else(|error| panic!("part {part:?} is admitted: {}", error.message()));
        let evaluator = RecognitionEvaluator::with_vision_provider(
            pack,
            Arc::new(FsAssetResolver::new(PathBuf::new())),
            Arc::new(FractionText),
        )
        .expect("the evaluator");

        let projection = evaluator
            .scene_context(&scene)
            .project_candidates(LAYOUT)
            .expect("the counter projects");

        let count = &projection.candidates()[0].features["count"];
        assert!(
            matches!(count, CandidateFeature::Integer { value, .. } if *value == expected),
            "part {part:?} reads {expected} from {FRACTION}: {count:?}"
        );
    }
}
