// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 B1 logic tests (slice BT-B1; B-train plan `rd-1021/train-0129/b-plan/plan.md`
// §2.1 items 1 and 2, test rows T1 and T5 of §5.3; register §1.163): a number's own dictionary
// turns a confused character into its digit while the evidence keeps the text as read, for a
// value-test side and for a reading mark alike; and a value-test side may declare an agreement,
// so its test takes several frames as a target under target consensus does, while the
// target-consensus refusals of value tests hold without an agreement. A scripted OCR provider
// answers each region with its own text on a neutral blank frame.

use crate::*;
use actingcommand_contract::{ResourceDeclarationReason, ResourceReadingDeclaration};
use serde_json::{Value, json};

const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const FRAME_WIDTH: u32 = 640;
const FRAME_HEIGHT: u32 = 360;
const COUNT: PackRect = PackRect {
    x: 10,
    y: 10,
    width: 80,
    height: 20,
};

// The count region reads `text` at confidence 0.9 as one block.
#[derive(Debug)]
struct CountText(&'static str);

impl VisionProvider for CountText {
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
        let text = if request.region == COUNT { self.0 } else { "" }.to_owned();
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: text.clone(),
            blocks: vec![OcrProviderTextBlock {
                text,
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

// The OCR target `ocr/count` over the count region, then the value test `check/count` whose left
// side reads it with `value` and, when given, `agreement`, and holds when it equals 100.
fn pack(value: Value, agreement: Option<Value>) -> Value {
    let mut left = json!({"target_id": "ocr/count", "trim": "whitespace_v1", "value": value});
    if let Some(agreement) = agreement {
        left["agreement"] = agreement;
    }
    json!({
        "schema_version": "0.7",
        "coordinate_space": {"width": FRAME_WIDTH, "height": FRAME_HEIGHT},
        "targets": [
            {"type": "ocr", "id": "ocr/count",
             "region": {"x": COUNT.x, "y": COUNT.y, "width": COUNT.width, "height": COUNT.height},
             "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact",
             "expected": ["unused"], "case_sensitive": false, "minimum_confidence": 0.8,
             "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256},
            {"type": "value_test", "id": "check/count", "left": left, "op": "eq",
             "right": {"const": 100}}
        ]
    })
}

fn evaluator(pack: &Value, text: &'static str) -> RecognitionEvaluator {
    RecognitionEvaluator::with_vision_provider(
        load_pack_from_json_str(&pack.to_string()).expect("the pack loads"),
        Arc::new(FsAssetResolver::new(PathBuf::new())),
        Arc::new(CountText(text)),
    )
    .expect("its evaluator")
}

// The declaration issue of a pack that does not load.
fn refusal(pack: &Value) -> (String, ResourceDeclarationReason) {
    let error = load_pack_from_json_str(&pack.to_string()).expect_err("the pack is refused");
    let issue = error.declaration_issue().expect("a declaration issue");
    (issue.field_path.clone(), issue.reason)
}

// T1 (plan §5.3 BT-B1 T1; §2.1 item 1): with the confusion `O` -> `0`, a value-test side that
// reads the text `1O0` reads 100, and its evidence keeps `1O0`; a reading mark with the same
// dictionary parses `1O0` as 100.
#[test]
fn bt_b1_t1_a_confusion_reads_its_digit_and_the_evidence_keeps_the_text() {
    let integer = json!({"type": "unsigned_integer", "min": 0, "max": 999,
                         "dictionary": {"confusions": [["O", "0"]]}});
    let evaluator = evaluator(&pack(integer.clone(), None), "1O0");
    let scene = Scene::from_rgb8(
        FRAME_WIDTH,
        FRAME_HEIGHT,
        &vec![0; (FRAME_WIDTH * FRAME_HEIGHT * 3) as usize],
    )
    .expect("blank scene");

    let evaluation = evaluator
        .evaluate_target(&scene, "check/count")
        .expect("the value test is evaluated");

    let left = &evaluation
        .value_test
        .as_ref()
        .expect("value test evidence")
        .data
        .left;
    assert_eq!(left.value, Some(100), "the dictionary turns O into 0");
    assert_eq!(left.raw_text.as_deref(), Some("1O0"), "the text as read");
    assert_eq!(left.problem, None);
    assert!(evaluation.passed, "100 eq 100");

    let mark: ResourceReadingDeclaration = serde_json::from_value(json!({
        "id": "count", "fact_key": "resource.count", "page_id": "page_a",
        "target_id": "ocr/count", "trim": "whitespace_v1", "value": integer,
        "minimum_confidence_milli": 900, "valid_for_ms": 60000
    }))
    .expect("a mark with a dictionary");
    assert!(mark.validate().is_ok(), "the mark is admitted");
    assert_eq!(
        mark.parse(" 1O0 "),
        Ok(100),
        "the mark's parser applies it too"
    );
}

// T5 (plan §5.3 BT-B1 T5; §2.1 item 2): a value-test side that declares an agreement is admitted,
// and the test's consumers take its three frames 40 ms apart as for target consensus; without an
// agreement the target-consensus refusals of value tests hold: a `target_consensus` declared on
// the test, and a side that reads a target under target consensus.
#[test]
fn bt_b1_t5_an_agreement_is_admitted_and_the_consensus_refusals_hold_without_it() {
    let integer = json!({"type": "unsigned_integer", "min": 0, "max": 999});
    let agreed = pack(
        integer.clone(),
        Some(json!({"frames": 3, "interval_ms": 40})),
    );
    let loaded =
        load_pack_from_json_str(&agreed.to_string()).expect("a side with an agreement is admitted");
    let evaluator = RecognitionEvaluator::with_vision_provider(
        loaded,
        Arc::new(FsAssetResolver::new(PathBuf::new())),
        Arc::new(CountText("100")),
    )
    .expect("its evaluator is built");
    assert_eq!(evaluator.target_sample_frames(), 3);
    assert_eq!(evaluator.target_sample_interval_ms(), 40);

    let consensus = json!({"samples": [{"frame": 0}, {"frame": 1}, {"frame": 2}], "k": 3,
                           "sample_interval_ms": 40});
    let mut declared = pack(integer.clone(), None);
    declared["target_consensus"] = json!({"check/count": consensus.clone()});
    assert_eq!(
        refusal(&declared),
        (
            "/target_consensus/check~1count".to_owned(),
            ResourceDeclarationReason::InvalidValue
        ),
        "a value test cannot declare target consensus"
    );
    let mut side = pack(integer, None);
    side["target_consensus"] = json!({"ocr/count": consensus});
    assert_eq!(
        refusal(&side),
        (
            "/targets/1/left/target_id".to_owned(),
            ResourceDeclarationReason::InvalidValue
        ),
        "a value test side reads a target without target consensus"
    );
}
