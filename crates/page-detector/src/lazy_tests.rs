// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #396 REC-6 (REC-M v1.2.1 §8.1, §8.3, §8.4, §12): located text in a page rule decides
// the page; a page that its immediate targets already rule out skips its lazy targets without a
// provider call; pages told apart only by lazy targets match as a full evaluation does; and a
// value test is refused in `forbidden`, directly or as a member of a check. A scripted OCR
// provider answers on synthetic neutral frames.

use super::*;
use actingcommand_contract::ResourceDeclarationReason;
use actingcommand_recognition_pack::{
    FsAssetResolver, NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, PackRect, VisionProvider, VisionProviderError, VisionProviderErrorCode,
    load_pack_from_json_str,
};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ALPHA_REGION: PackRect = PackRect {
    x: 10,
    y: 40,
    width: 200,
    height: 40,
};
const BETA_REGION: PackRect = PackRect {
    x: 10,
    y: 120,
    width: 200,
    height: 40,
};

// Reads the configured words of a region, one block each, and counts every call.
#[derive(Debug)]
struct Words {
    regions: Vec<(PackRect, Vec<&'static str>)>,
    calls: AtomicUsize,
}

impl Words {
    fn new(regions: Vec<(PackRect, Vec<&'static str>)>) -> Arc<Self> {
        Arc::new(Self {
            regions,
            calls: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl VisionProvider for Words {
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
        let words = self
            .regions
            .iter()
            .find(|(region, _)| *region == request.region)
            .map(|(_, words)| words.clone())
            .unwrap_or_default();
        let blocks = words
            .iter()
            .enumerate()
            .map(|(index, word)| OcrProviderTextBlock {
                text: (*word).to_owned(),
                rect: PackRect {
                    x: request.region.x + 60 * index as i32,
                    y: request.region.y,
                    width: 50,
                    height: 20,
                },
                confidence: Some(0.9),
            })
            .collect::<Vec<_>>();
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: words.join("\n"),
            blocks,
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

fn locate(id: &str, region: PackRect, expected: &str) -> Value {
    json!({
        "type": "ocr", "id": id,
        "region": {"x": region.x, "y": region.y, "width": region.width, "height": region.height},
        "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact", "expected": [expected],
        "case_sensitive": false, "minimum_confidence": 0.8, "model_ref": "PP-OCRv6_medium",
        "model_sha256": MODEL_SHA256,
        "locate": {"normalize": "identity_v1", "blocks": "detect", "line_merge": {"gap_factor": 0}}
    })
}

// A red pixel at (0, 0) shows the page marker `page/a`.
fn evaluator(extra: Vec<Value>, provider: Arc<Words>) -> RecognitionEvaluator {
    let mut targets = vec![
        json!({"type": "color", "id": "page/a", "region": {"x": 0, "y": 0, "width": 1, "height": 1},
               "expected": [255, 0, 0], "max_distance": 0.0}),
        locate("text/alpha", ALPHA_REGION, "ALPHA"),
        locate("text/beta", BETA_REGION, "BETA"),
    ];
    targets.extend(extra);
    let pack = load_pack_from_json_str(
        &json!({
            "schema_version": "0.7",
            "coordinate_space": {"width": 640, "height": 360},
            "targets": targets
        })
        .to_string(),
    )
    .expect("recognition pack");
    RecognitionEvaluator::with_vision_provider(
        pack,
        Arc::new(FsAssetResolver::new(PathBuf::new())),
        provider,
    )
    .expect("recognition evaluator")
}

fn detector(pages: Value) -> PageDetector {
    PageDetector::new(
        serde_json::from_value(json!({"schema_version": "0.6", "pages": pages})).expect("page set"),
    )
    .expect("page detector")
}

fn frame(marker: bool) -> Scene {
    let mut pixels = vec![0_u8; 640 * 360 * 3];
    if marker {
        pixels[0..3].copy_from_slice(&[255, 0, 0]);
    }
    Scene::from_rgb8(640, 360, &pixels).expect("scene")
}

fn matched_pages(detector: &PageDetector, evaluator: &RecognitionEvaluator) -> Vec<String> {
    detector
        .evaluate_all(evaluator, &frame(true))
        .expect("page recognition")
        .into_iter()
        .filter(|page| page.matched)
        .map(|page| page.page_id)
        .collect()
}

#[test]
fn a_locate_target_in_required_matches_the_page_only_with_its_text() {
    let pages = json!([{"id": "neutral/detail", "required": ["page/a", "text/alpha"]}]);
    for (words, matched) in [(vec!["GAMMA", "ALPHA"], true), (vec!["GAMMA"], false)] {
        let provider = Words::new(vec![(ALPHA_REGION, words.clone())]);
        let evaluator = evaluator(Vec::new(), provider.clone());

        let page = detector(pages.clone())
            .evaluate_page(&evaluator, &frame(true), "neutral/detail")
            .expect("page evaluation");

        assert_eq!(page.matched, matched, "{words:?}");
        let text = &page.target_results[1];
        assert_eq!(text.target_id, "text/alpha");
        assert_eq!(text.passed, matched);
        assert_eq!(text.skipped, None);
        assert_eq!(
            text.evaluation
                .failure
                .as_ref()
                .map(|failure| failure.code().as_str()),
            (!matched).then_some("text_locate_not_found")
        );
        assert_eq!(provider.calls(), 1);
    }
}

#[test]
fn a_page_ruled_out_by_its_immediate_targets_skips_its_lazy_targets_without_a_provider_call() {
    let provider = Words::new(vec![
        (ALPHA_REGION, vec!["ALPHA"]),
        (BETA_REGION, vec!["BETA"]),
    ]);
    let unread = evaluator(Vec::new(), provider.clone());
    let detector = detector(json!([{
        "id": "neutral/detail", "required": ["page/a", "text/alpha"], "optional": ["text/beta"]
    }]));

    // The marker is absent: the immediate required target fails.
    let page = detector
        .evaluate_page(&unread, &frame(false), "neutral/detail")
        .expect("page evaluation");

    assert!(!page.matched);
    assert_eq!(provider.calls(), 0, "no OCR for a page that cannot match");
    for lazy in &page.target_results[1..] {
        assert_eq!(lazy.skipped, Some(TaskDiagnosticTargetSkip::PageUnmatched));
        assert!(!lazy.passed);
        assert!(!lazy.evaluation.passed);
        assert_eq!(lazy.evaluation.ocr, None);
    }
    assert_eq!(
        page.target_results
            .iter()
            .map(|target| (target.target_id.as_str(), target.role))
            .collect::<Vec<_>>(),
        [
            ("page/a", PageTargetRole::Required),
            ("text/alpha", PageTargetRole::Required),
            ("text/beta", PageTargetRole::Optional),
        ],
        "results keep the declared order"
    );

    // A lazy required target that fails rules out the lazy targets after it.
    let provider = Words::new(vec![
        (ALPHA_REGION, vec!["GAMMA"]),
        (BETA_REGION, vec!["BETA"]),
    ]);
    let failing = evaluator(Vec::new(), provider.clone());
    let page = detector
        .evaluate_page(&failing, &frame(true), "neutral/detail")
        .expect("page evaluation");
    assert!(!page.matched);
    assert_eq!(provider.calls(), 1, "only the required text is read");
    assert_eq!(page.target_results[1].skipped, None);
    assert_eq!(
        page.target_results[2].skipped,
        Some(TaskDiagnosticTargetSkip::PageUnmatched)
    );
}

#[test]
fn pages_told_apart_only_by_lazy_targets_match_as_a_full_evaluation_does() {
    let detector = detector(json!([
        {"id": "neutral/one", "required": ["page/a", "text/alpha"]},
        {"id": "neutral/two", "required": ["page/a", "text/beta"]}
    ]));
    for (alpha, beta, expected) in [
        (vec!["ALPHA"], vec![], vec!["neutral/one"]),
        (vec![], vec!["BETA"], vec!["neutral/two"]),
        (
            vec!["ALPHA"],
            vec!["BETA"],
            vec!["neutral/one", "neutral/two"],
        ),
        (vec![], vec![], vec![]),
    ] {
        let provider = Words::new(vec![
            (ALPHA_REGION, alpha.clone()),
            (BETA_REGION, beta.clone()),
        ]);
        let evaluator = evaluator(Vec::new(), provider.clone());

        assert_eq!(
            matched_pages(&detector, &evaluator),
            expected,
            "{alpha:?} {beta:?}"
        );
        assert_eq!(provider.calls(), 2, "each page reads its own text");
    }
}

#[test]
fn a_value_test_in_forbidden_is_refused_directly_or_as_a_check_member() {
    let value_test = json!({
        "type": "value_test", "id": "check/count",
        "left": {"target_id": "text/count", "trim": "whitespace_v1",
                 "value": {"type": "unsigned_integer", "min": 0, "max": 99}},
        "op": "gt", "right": {"const": 0}
    });
    let plain = json!({
        "type": "ocr", "id": "text/count",
        "region": {"x": 300, "y": 40, "width": 80, "height": 20},
        "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact", "expected": ["unused"],
        "case_sensitive": false, "minimum_confidence": 0.8, "model_ref": "PP-OCRv6_medium",
        "model_sha256": MODEL_SHA256
    });
    let check = json!({"type": "composite", "id": "check/any", "mode": "any_of",
                       "members": ["page/a", "check/count"]});
    let evaluator = evaluator(vec![plain, value_test, check], Words::new(Vec::new()));

    for (forbidden, pointer) in [
        ("check/count", "/pages/0/forbidden/0"),
        ("check/any", "/pages/0/forbidden/0"),
    ] {
        let error = detector(json!([
            {"id": "neutral/detail", "required": ["page/a"], "forbidden": [forbidden]}
        ]))
        .validate(&evaluator)
        .expect_err("a value test is never forbidden");
        let issue = error.declaration_issue().expect("the refused declaration");
        assert_eq!(issue.field_path, pointer, "{forbidden}");
        assert_eq!(issue.reason, ResourceDeclarationReason::InvalidValue);
    }

    detector(json!([
        {"id": "neutral/detail", "required": ["page/a", "check/count"], "optional": ["check/any"]}
    ]))
    .validate(&evaluator)
    .expect("a value test is admitted in required and optional");
}
