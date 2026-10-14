// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #396 REC-6 logic tests of value tests (REC-M v1.2.1 §8.4, §12): the seven
// comparisons, a right side scaled by mul and add, `parses`, the clock_hms format, the
// whitespace_all_v1 trim with both parts of current_capacity, and a side that cannot be read.
// A scripted OCR provider answers each region with its own text on a synthetic neutral frame.

use crate::*;
use actingcommand_contract::outcome::Category;
use actingcommand_contract::{
    TaskDiagnosticValueOperand, TaskDiagnosticValueRight, TaskDiagnosticValueTestData,
};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};

const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const FRAME_WIDTH: u32 = 640;
const FRAME_HEIGHT: u32 = 360;

// Each region reads its own text and confidence as one block; every call is counted.
struct RegionTexts {
    texts: Vec<(PackRect, String, Option<f32>)>,
    calls: AtomicUsize,
}

impl fmt::Debug for RegionTexts {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RegionTexts")
    }
}

impl RegionTexts {
    fn new(texts: &[(PackRect, &str, Option<f32>)]) -> Arc<Self> {
        Arc::new(Self {
            texts: texts
                .iter()
                .map(|(region, text, confidence)| (*region, (*text).to_owned(), *confidence))
                .collect(),
            calls: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl VisionProvider for RegionTexts {
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
        self.calls.fetch_add(1, Ordering::SeqCst);
        let (text, confidence) = self
            .texts
            .iter()
            .find(|(region, _, _)| *region == request.region)
            .map(|(_, text, confidence)| (text.clone(), *confidence))
            .unwrap_or_default();
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: text.clone(),
            blocks: vec![OcrProviderTextBlock {
                text,
                rect: request.region,
                confidence,
            }],
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

const LEFT: PackRect = PackRect {
    x: 10,
    y: 10,
    width: 80,
    height: 20,
};
const RIGHT: PackRect = PackRect {
    x: 110,
    y: 10,
    width: 80,
    height: 20,
};

fn ocr_target(id: &str, region: PackRect) -> Value {
    json!({
        "type": "ocr", "id": id,
        "region": {"x": region.x, "y": region.y, "width": region.width, "height": region.height},
        "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact", "expected": ["unused"],
        "case_sensitive": false, "minimum_confidence": 0.8, "model_ref": "PP-OCRv6_medium",
        "model_sha256": MODEL_SHA256
    })
}

fn side(target_id: &str, trim: &str, value: Value) -> Value {
    json!({"target_id": target_id, "trim": trim, "value": value})
}

fn integer(min: u64, max: u64) -> Value {
    json!({"type": "unsigned_integer", "min": min, "max": max})
}

fn value_test(left: Value, op: &str, right: Option<Value>) -> Value {
    let mut test = json!({"type": "value_test", "id": "check/values", "left": left, "op": op});
    if let Some(right) = right {
        test["right"] = right;
    }
    test
}

fn evaluator(test: Value, provider: Arc<RegionTexts>) -> RecognitionEvaluator {
    let pack = load_pack_from_json_str(
        &json!({
            "schema_version": "0.7",
            "coordinate_space": {"width": FRAME_WIDTH, "height": FRAME_HEIGHT},
            "targets": [ocr_target("ocr/left", LEFT), ocr_target("ocr/right", RIGHT), test]
        })
        .to_string(),
    )
    .expect("value test pack loads");
    RecognitionEvaluator::with_vision_provider(
        pack,
        Arc::new(FsAssetResolver::new(PathBuf::new())),
        provider,
    )
    .expect("value test evaluator")
}

fn evaluate(evaluator: &RecognitionEvaluator) -> TargetEvaluation {
    let scene = Scene::from_rgb8(
        FRAME_WIDTH,
        FRAME_HEIGHT,
        &vec![0; (FRAME_WIDTH * FRAME_HEIGHT * 3) as usize],
    )
    .expect("blank scene");
    evaluator
        .evaluate_target(&scene, "check/values")
        .expect("the value test is evaluated")
}

fn evidence(evaluation: &TargetEvaluation) -> &TaskDiagnosticValueTestData {
    &evaluation
        .value_test
        .as_ref()
        .expect("value test evidence")
        .data
}

#[test]
fn each_comparison_holds_or_fails_with_value_test_not_met() {
    for (op, constant, holds) in [
        ("eq", 5, true),
        ("eq", 6, false),
        ("ne", 6, true),
        ("ne", 5, false),
        ("lt", 6, true),
        ("lt", 5, false),
        ("le", 5, true),
        ("le", 4, false),
        ("gt", 4, true),
        ("gt", 5, false),
        ("ge", 5, true),
        ("ge", 6, false),
    ] {
        let provider = RegionTexts::new(&[(LEFT, "5", Some(0.9))]);
        let evaluator = evaluator(
            value_test(
                side("ocr/left", "whitespace_v1", integer(0, 99)),
                op,
                Some(json!({"const": constant})),
            ),
            provider,
        );

        let evaluation = evaluate(&evaluator);

        assert_eq!(evaluation.passed, holds, "5 {op} {constant}");
        assert_eq!(evaluation.kind, TargetKind::ValueTest);
        let data = evidence(&evaluation);
        assert_eq!(data.passed, holds);
        assert_eq!(data.left.value, Some(5));
        assert_eq!(
            data.right,
            Some(TaskDiagnosticValueRight::Const { constant })
        );
        match &evaluation.failure {
            None => assert!(
                holds,
                "5 {op} {constant} names no failure only when it holds"
            ),
            Some(failure) => {
                assert!(!holds);
                assert_eq!(failure.code().as_str(), "value_test_not_met");
                assert_eq!(failure.category(), Category::Error);
                assert_eq!(failure.value("target_id"), Some(&json!("check/values")));
                assert_eq!(failure.value("value_op"), Some(&json!(op)));
                assert_eq!(failure.value("stage"), Some(&json!("value_test_evaluate")));
            }
        }
    }

    // The seventh comparison: `parses` reads the left side only.
    let provider = RegionTexts::new(&[(LEFT, "5", Some(0.9))]);
    let evaluation = evaluate(&evaluator(
        value_test(
            side("ocr/left", "whitespace_v1", integer(0, 99)),
            "parses",
            None,
        ),
        provider.clone(),
    ));
    assert!(evaluation.passed);
    assert_eq!(evidence(&evaluation).right, None);
    assert_eq!(provider.calls(), 1);
}

#[test]
fn the_right_side_is_another_read_value_times_mul_plus_add() {
    // A cost that is 20 times a quantity: 60 = 3 * 20 + 0, but not 3 * 20 + 1.
    for (add, holds) in [(0, true), (1, false)] {
        let provider = RegionTexts::new(&[(LEFT, "60", Some(0.9)), (RIGHT, "3", Some(0.9))]);
        let mut right = side("ocr/right", "whitespace_v1", integer(0, 99));
        right["mul"] = json!(20);
        right["add"] = json!(add);
        let evaluation = evaluate(&evaluator(
            value_test(
                side("ocr/left", "whitespace_v1", integer(0, 9999)),
                "eq",
                Some(right),
            ),
            provider.clone(),
        ));

        assert_eq!(evaluation.passed, holds, "60 eq 3 * 20 + {add}");
        assert_eq!(
            evidence(&evaluation).right,
            Some(TaskDiagnosticValueRight::Operand {
                operand: TaskDiagnosticValueOperand {
                    target_id: "ocr/right".to_owned(),
                    raw_text: Some("3".to_owned()),
                    value: Some(3),
                    problem: None,
                },
                mul: 20,
                add,
            })
        );
        assert_eq!(provider.calls(), 2, "each side reads once");
    }
}

#[test]
fn parses_with_clock_hms_holds_for_a_time_and_names_a_malformed_one() {
    for (text, seconds) in [
        ("1:05:00", Some(3_900)),
        ("1:5:00", None),
        ("1:05:60", None),
    ] {
        let provider = RegionTexts::new(&[(LEFT, text, Some(0.9))]);
        let evaluation = evaluate(&evaluator(
            value_test(
                side(
                    "ocr/left",
                    "whitespace_v1",
                    json!({"type": "unsigned_integer", "min": 0, "max": 359_999,
                           "format": "clock_hms"}),
                ),
                "parses",
                None,
            ),
            provider,
        ));

        assert_eq!(evaluation.passed, seconds.is_some(), "{text}");
        assert_eq!(evidence(&evaluation).left.value, seconds, "{text}");
        if seconds.is_none() {
            let failure = evaluation.failure.as_ref().expect("failure");
            assert_eq!(failure.code().as_str(), "value_test_unresolved");
            assert_eq!(failure.value("value_side"), Some(&json!("left")));
            assert_eq!(failure.value("value_problem"), Some(&json!("invalid")));
        }
    }
}

#[test]
fn whitespace_all_v1_reads_both_parts_of_a_spaced_current_capacity() {
    // "1 / 3": the numerator below the denominator. The same OCR target is both sides; one
    // read serves both.
    let provider = RegionTexts::new(&[(LEFT, "1 / 3", Some(0.9))]);
    let current_capacity =
        json!({"type": "unsigned_integer", "min": 0, "max": 99, "format": "current_capacity"});
    let mut left = side("ocr/left", "whitespace_all_v1", current_capacity.clone());
    left["part"] = json!("current");
    let mut right = side("ocr/left", "whitespace_all_v1", current_capacity);
    right["part"] = json!("capacity");
    right["mul"] = json!(1);
    right["add"] = json!(0);

    let evaluation = evaluate(&evaluator(
        value_test(left, "lt", Some(right)),
        provider.clone(),
    ));

    assert!(evaluation.passed);
    let data = evidence(&evaluation);
    assert_eq!(data.left.value, Some(1));
    assert_eq!(data.left.raw_text.as_deref(), Some("1 / 3"));
    let Some(TaskDiagnosticValueRight::Operand { operand, .. }) = &data.right else {
        panic!("the right side reads a value");
    };
    assert_eq!(operand.value, Some(3));
    assert_eq!(provider.calls(), 1);
}

#[test]
fn a_side_that_cannot_be_read_fails_with_value_test_unresolved_naming_side_and_problem() {
    let scaled = |target: &str| {
        let mut right = side(target, "whitespace_v1", integer(0, 99));
        right["mul"] = json!(1);
        right["add"] = json!(0);
        right
    };
    for (left_text, left_confidence, right_text, side_name, problem) in [
        (" ", Some(0.9), "5", "left", "empty"),
        ("5a", Some(0.9), "5", "left", "invalid"),
        ("18446744073709551616", Some(0.9), "5", "left", "overflow"),
        ("100", Some(0.9), "5", "left", "out_of_range"),
        ("5", Some(0.5), "5", "left", "low_confidence"),
        ("5", None, "5", "left", "low_confidence"),
        ("5", Some(0.9), "x", "right", "invalid"),
    ] {
        let provider = RegionTexts::new(&[
            (LEFT, left_text, left_confidence),
            (RIGHT, right_text, Some(0.9)),
        ]);
        let evaluation = evaluate(&evaluator(
            value_test(
                side("ocr/left", "whitespace_v1", integer(0, 99)),
                "eq",
                Some(scaled("ocr/right")),
            ),
            provider,
        ));

        assert!(!evaluation.passed, "{left_text:?} {right_text:?}");
        let failure = evaluation.failure.as_ref().expect("failure");
        assert_eq!(failure.code().as_str(), "value_test_unresolved");
        assert_eq!(failure.category(), Category::Error);
        assert_eq!(failure.value("target_id"), Some(&json!("check/values")));
        assert_eq!(failure.value("value_side"), Some(&json!(side_name)));
        assert_eq!(failure.value("value_problem"), Some(&json!(problem)));
        assert_eq!(failure.value("stage"), Some(&json!("value_test_evaluate")));
        let data = evidence(&evaluation);
        let unread = match side_name {
            "left" => data.left.clone(),
            _ => match &data.right {
                Some(TaskDiagnosticValueRight::Operand { operand, .. }) => operand.clone(),
                other => panic!("the right side reads a value, got {other:?}"),
            },
        };
        assert_eq!(unread.value, None);
        assert_eq!(
            serde_json::to_value(unread.problem).expect("problem"),
            json!(problem)
        );
    }
}
