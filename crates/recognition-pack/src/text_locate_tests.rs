// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #396 REC-1 logic tests: located text, its cells, the per-request memory and the
//! per-consumer budget, with a scripted OCR provider and synthetic neutral frames.

use crate::*;
use actingcommand_contract::ResourceDeclarationIssue;
use serde_json::{Value, json};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use actingcommand_contract::outcome::Category;

const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const FRAME_WIDTH: u32 = 640;
const FRAME_HEIGHT: u32 = 360;

type Respond = dyn Fn(PackRect) -> OcrProviderResult + Send + Sync;

/// Answers every OCR request from its region and records each call.
struct ScriptedOcr {
    respond: Box<Respond>,
    calls: AtomicUsize,
    requests: Mutex<Vec<(PackRect, OcrProviderBlocks)>>,
}

impl fmt::Debug for ScriptedOcr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ScriptedOcr")
    }
}

impl ScriptedOcr {
    fn new(respond: impl Fn(PackRect) -> OcrProviderResult + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            respond: Box::new(respond),
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
        })
    }

    /// The same blocks for every request.
    fn blocks(blocks: Vec<(&str, PackRect, Option<f32>)>) -> Arc<Self> {
        let blocks = blocks
            .into_iter()
            .map(|(text, rect, confidence)| (text.to_owned(), rect, confidence))
            .collect::<Vec<_>>();
        Self::new(move |_| result(&blocks, None))
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
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
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests
            .lock()
            .expect("request log")
            .push((request.region, request.blocks));
        Ok((self.respond)(request.region))
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

fn result(
    blocks: &[(String, PackRect, Option<f32>)],
    detection: Option<OcrProviderDetection>,
) -> OcrProviderResult {
    OcrProviderResult {
        ppocr_diagnostics: Vec::new(),
        text: blocks
            .iter()
            .map(|(text, _, _)| text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        blocks: blocks
            .iter()
            .map(|(text, rect, confidence)| OcrProviderTextBlock {
                text: text.clone(),
                rect: *rect,
                confidence: *confidence,
            })
            .collect(),
        confidence: Some(0.875),
        detection,
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

/// The list region of the fixtures; its cells are 120 px squares at a 140 px pitch, 4 by 2.
fn list_region() -> PackRect {
    rect(40, 60, 560, 280)
}

fn region_json(region: PackRect) -> Value {
    json!({"x": region.x, "y": region.y, "width": region.width, "height": region.height})
}

fn detect(gap_factor: u32) -> Value {
    json!({"normalize": "identity_v1", "blocks": "detect", "line_merge": {"gap_factor": gap_factor}})
}

fn cells() -> Value {
    json!({"x": 0, "y": 0, "width": 120, "height": 120, "pitch_x": 140, "pitch_y": 140,
           "columns": 4, "rows": 2})
}

fn ocr_target(
    id: &str,
    region: Value,
    match_mode: &str,
    expected: &[&str],
    locate: Option<Value>,
) -> Value {
    let mut target = json!({
        "type": "ocr", "id": id, "region": region, "languages": ["en"], "timeout_ms": 1000,
        "match_mode": match_mode, "expected": expected, "case_sensitive": false,
        "minimum_confidence": 0.8, "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256
    });
    if let Some(locate) = locate {
        target["locate"] = locate;
    }
    target
}

fn locate_target(id: &str, expected: &[&str], match_mode: &str, locate: Value) -> Value {
    ocr_target(
        id,
        region_json(list_region()),
        match_mode,
        expected,
        Some(locate),
    )
}

fn pack_json(targets: Vec<Value>) -> Value {
    json!({
        "schema_version": "0.7",
        "coordinate_space": {"width": FRAME_WIDTH, "height": FRAME_HEIGHT},
        "targets": targets
    })
}

fn evaluator(pack: Value, provider: Arc<ScriptedOcr>) -> RecognitionEvaluator {
    let pack = load_pack_from_json_str(&pack.to_string()).expect("locate pack loads");
    RecognitionEvaluator::with_vision_provider(
        pack,
        Arc::new(FsAssetResolver::new(PathBuf::new())),
        provider,
    )
    .expect("locate evaluator")
}

fn scene() -> Scene {
    Scene::from_rgb8(
        FRAME_WIDTH,
        FRAME_HEIGHT,
        &vec![0; (FRAME_WIDTH * FRAME_HEIGHT * 3) as usize],
    )
    .expect("blank scene")
}

fn evaluate(evaluator: &RecognitionEvaluator, target_id: &str) -> TargetEvaluation {
    evaluator
        .evaluate_target(&scene(), target_id)
        .expect("evaluated")
}

fn located(evaluation: &TargetEvaluation) -> OcrLocatedEvidence {
    evaluation
        .ocr
        .as_ref()
        .and_then(|ocr| ocr.located)
        .expect("located evidence")
}

fn failure_code(evaluation: &TargetEvaluation) -> &str {
    evaluation
        .failure
        .as_ref()
        .expect("a typed failure")
        .code()
        .as_str()
}

#[test]
fn a_text_found_in_its_region_gives_its_block_and_evidence() {
    let provider = ScriptedOcr::blocks(vec![
        ("GAMMA", rect(60, 80, 60, 20), Some(0.9375)),
        ("ALPHA", rect(190, 240, 80, 20), Some(0.875)),
    ]);
    let evaluator = evaluator(
        pack_json(vec![locate_target(
            "text/entry",
            &["ALPHA", "ALPHA 1"],
            "exact",
            detect(0),
        )]),
        provider.clone(),
    );

    let evaluation = evaluate(&evaluator, "text/entry");

    assert!(evaluation.passed);
    assert_eq!(evaluation.failure, None);
    assert_eq!(
        located(&evaluation),
        OcrLocatedEvidence {
            block_index: 1,
            rect: rect(190, 240, 80, 20),
            confidence_milli: 875,
            cell_index: None,
            cell_rect: None,
            truncated: false,
        }
    );
    let ocr = evaluation.ocr.as_ref().expect("OCR evidence");
    assert_eq!(ocr.matched_expected.as_deref(), Some("ALPHA"));
    assert_eq!(
        *provider.requests.lock().expect("request log"),
        [(
            list_region(),
            OcrProviderBlocks::Detect {
                line_merge_gap_factor: Some(0)
            }
        )]
    );
}

#[test]
fn a_missing_text_does_not_pass_and_names_text_locate_not_found() {
    let provider = ScriptedOcr::blocks(vec![
        ("GAMMA", rect(60, 80, 60, 20), Some(0.9375)),
        ("DELTA", rect(190, 240, 80, 20), Some(0.9375)),
    ]);
    let evaluator = evaluator(
        pack_json(vec![locate_target(
            "text/entry",
            &["ALPHA"],
            "exact",
            detect(0),
        )]),
        provider,
    );

    let evaluation = evaluate(&evaluator, "text/entry");

    assert!(!evaluation.passed);
    assert_eq!(failure_code(&evaluation), "text_locate_not_found");
    let failure = evaluation.failure.as_ref().expect("failure");
    assert_eq!(failure.category(), Category::Error);
    assert_eq!(failure.value("target_id"), Some(&json!("text/entry")));
    assert_eq!(failure.value("count"), Some(&json!(2)));
    assert_eq!(failure.value("stage"), Some(&json!("text_locate_evaluate")));
    assert_eq!(evaluation.ocr.as_ref().and_then(|ocr| ocr.located), None);
}

#[test]
fn full_width_case_and_spaces_do_not_change_a_hit() {
    let provider = ScriptedOcr::blocks(vec![(
        "\u{ff21}\u{ff4c}\u{ff50}\u{ff48}\u{ff41}  \u{ff11}",
        rect(190, 240, 80, 20),
        Some(0.875),
    )]);
    let evaluator = evaluator(
        pack_json(vec![locate_target(
            "text/entry",
            &["alpha 1"],
            "exact",
            detect(0),
        )]),
        provider,
    );

    let evaluation = evaluate(&evaluator, "text/entry");

    assert!(evaluation.passed);
    assert_eq!(located(&evaluation).rect, rect(190, 240, 80, 20));
}

#[test]
fn exact_and_contains_compare_each_block_on_its_own() {
    let whole = || ScriptedOcr::blocks(vec![("ALPHA 1", rect(190, 240, 80, 20), Some(0.875))]);
    let exact = evaluator(
        pack_json(vec![locate_target(
            "text/entry",
            &["ALPHA"],
            "exact",
            detect(0),
        )]),
        whole(),
    );
    assert!(
        !evaluate(&exact, "text/entry").passed,
        "exact needs the whole block"
    );
    let contains = evaluator(
        pack_json(vec![locate_target(
            "text/entry",
            &["ALPHA"],
            "contains",
            detect(0),
        )]),
        whole(),
    );
    assert!(
        evaluate(&contains, "text/entry").passed,
        "contains finds it in the block"
    );

    let split = evaluator(
        pack_json(vec![locate_target(
            "text/entry",
            &["ALPHA"],
            "contains",
            detect(0),
        )]),
        ScriptedOcr::blocks(vec![
            ("AL", rect(190, 240, 30, 20), Some(0.875)),
            ("PHA", rect(222, 240, 40, 20), Some(0.875)),
        ]),
    );
    let evaluation = evaluate(&split, "text/entry");
    assert!(
        !evaluation.passed,
        "a word split over two blocks is not found"
    );
    assert_eq!(failure_code(&evaluation), "text_locate_not_found");
}

#[test]
fn a_block_beyond_128_characters_matches_nothing() {
    let long = "A".repeat(129);
    let evaluator = evaluator(
        pack_json(vec![locate_target(
            "text/entry",
            &["A"],
            "contains",
            detect(0),
        )]),
        ScriptedOcr::blocks(vec![(long.as_str(), rect(190, 240, 80, 20), Some(0.875))]),
    );

    let evaluation = evaluate(&evaluator, "text/entry");

    assert!(!evaluation.passed);
    assert_eq!(failure_code(&evaluation), "text_locate_not_found");
}

#[test]
fn a_match_without_enough_confidence_is_not_found() {
    for confidence in [Some(0.5), None] {
        let evaluator = evaluator(
            pack_json(vec![locate_target(
                "text/entry",
                &["ALPHA"],
                "exact",
                detect(0),
            )]),
            ScriptedOcr::blocks(vec![("ALPHA", rect(190, 240, 80, 20), confidence)]),
        );

        let evaluation = evaluate(&evaluator, "text/entry");

        assert!(!evaluation.passed, "confidence {confidence:?}");
        assert_eq!(failure_code(&evaluation), "text_locate_not_found");
    }
}

#[test]
fn declared_cells_choose_the_cell_that_holds_the_block_center() {
    let mut locate = detect(0);
    locate["cells"] = cells();
    let in_cell = evaluator(
        pack_json(vec![locate_target(
            "text/entry",
            &["ALPHA"],
            "exact",
            locate.clone(),
        )]),
        ScriptedOcr::blocks(vec![("ALPHA", rect(190, 240, 80, 20), Some(0.875))]),
    );

    let evaluation = evaluate(&in_cell, "text/entry");

    assert!(evaluation.passed);
    let found = located(&evaluation);
    assert_eq!(found.cell_index, Some(5));
    assert_eq!(found.cell_rect, Some(rect(180, 200, 120, 120)));
    assert_eq!(found.rect, rect(190, 240, 80, 20));

    // The center (160, 105) lies in the gap between the first two columns.
    let between = evaluator(
        pack_json(vec![locate_target(
            "text/entry",
            &["ALPHA"],
            "exact",
            locate,
        )]),
        ScriptedOcr::blocks(vec![("ALPHA", rect(150, 100, 20, 10), Some(0.875))]),
    );
    let evaluation = evaluate(&between, "text/entry");
    assert!(!evaluation.passed);
    assert_eq!(failure_code(&evaluation), "text_locate_outside_cells");
    let failure = evaluation.failure.as_ref().expect("failure");
    assert_eq!(failure.category(), Category::Error);
    assert_eq!(failure.value("target_id"), Some(&json!("text/entry")));
    assert_eq!(failure.value("stage"), Some(&json!("text_locate_evaluate")));
}

#[test]
fn truncated_detection_refuses_a_miss_and_records_a_hit() {
    let truncated = |text: &'static str| {
        ScriptedOcr::new(move |_| {
            result(
                &[(text.to_owned(), rect(190, 240, 80, 20), Some(0.875))],
                Some(OcrProviderDetection {
                    detected: 65,
                    limit: 64,
                }),
            )
        })
    };
    let missed = evaluator(
        pack_json(vec![locate_target(
            "text/entry",
            &["ALPHA"],
            "exact",
            detect(0),
        )]),
        truncated("DELTA"),
    );

    let error = missed
        .evaluate_target(&scene(), "text/entry")
        .expect_err("a miss among truncated boxes cannot conclude");

    assert_eq!(error.code(), RecognitionPackErrorCode::TextReadRefused);
    let outcome = error.outcome().expect("typed outcome");
    assert!(outcome.is(RecognitionPackCode::TextReadBlocksTruncated));
    assert_eq!(outcome.category(), Category::Error);
    assert_eq!(outcome.value("stage"), Some(&json!("text_locate_evaluate")));
    assert_eq!(outcome.value("target_id"), Some(&json!("text/entry")));
    assert_eq!(outcome.value("count"), Some(&json!(65)));
    assert_eq!(outcome.value("limit_count"), Some(&json!(64)));

    let hit = evaluator(
        pack_json(vec![locate_target(
            "text/entry",
            &["ALPHA"],
            "exact",
            detect(0),
        )]),
        truncated("ALPHA"),
    );
    let evaluation = evaluate(&hit, "text/entry");
    assert!(evaluation.passed);
    assert!(located(&evaluation).truncated);
}

#[test]
fn targets_that_share_a_request_call_the_provider_once_per_frame() {
    let provider = ScriptedOcr::blocks(vec![
        ("ALPHA", rect(190, 240, 80, 20), Some(0.875)),
        ("BETA", rect(330, 240, 80, 20), Some(0.875)),
    ]);
    let evaluator = evaluator(
        pack_json(vec![
            locate_target("text/first", &["ALPHA"], "exact", detect(0)),
            locate_target("text/second", &["BETA"], "exact", detect(0)),
        ]),
        provider.clone(),
    );
    let scene = scene();
    let context = evaluator.scene_context(&scene);

    let first = context.evaluate_target("text/first").expect("first");
    let second = context.evaluate_target("text/second").expect("second");

    assert!(first.passed && second.passed);
    assert_eq!(located(&second).rect, rect(330, 240, 80, 20));
    assert_eq!(provider.calls(), 1);
}

#[test]
fn a_fifth_call_and_an_area_beyond_the_frame_are_refused_before_the_provider_is_called() {
    let provider =
        ScriptedOcr::new(|region| result(&[("ALPHA".to_owned(), region, Some(0.875))], None));
    let small = |index: i32| {
        ocr_target(
            &format!("text/{index}"),
            region_json(rect(index * 100, 0, 80, 40)),
            "exact",
            &["ALPHA"],
            Some(detect(0)),
        )
    };
    let calls = evaluator(pack_json((0..5).map(small).collect()), provider.clone());
    let scene = scene();
    let context = calls.scene_context(&scene);
    for index in 0..4 {
        assert!(
            context
                .evaluate_target(&format!("text/{index}"))
                .expect("within budget")
                .passed
        );
    }

    let error = context
        .evaluate_target("text/4")
        .expect_err("the fifth call exceeds the budget");

    assert_eq!(provider.calls(), 4);
    let outcome = error.outcome().expect("typed outcome");
    assert!(outcome.is(RecognitionPackCode::TextReadBudgetExceeded));
    assert_eq!(outcome.category(), Category::Error);
    assert_eq!(outcome.value("stage"), Some(&json!("text_read_request")));
    assert_eq!(outcome.value("text_read_budget"), Some(&json!("calls")));
    assert_eq!(outcome.value("count"), Some(&json!(4)));
    assert_eq!(outcome.value("limit_count"), Some(&json!(4)));
    assert_eq!(outcome.value("target_id"), Some(&json!("text/4")));

    let provider =
        ScriptedOcr::new(|region| result(&[("ALPHA".to_owned(), region, Some(0.875))], None));
    let wide = |id: &str, y: i32| {
        ocr_target(
            id,
            region_json(rect(0, y, 640, 240)),
            "exact",
            &["ALPHA"],
            Some(detect(0)),
        )
    };
    let area = evaluator(
        pack_json(vec![wide("text/upper", 0), wide("text/lower", 120)]),
        provider.clone(),
    );
    let context = area.scene_context(&scene);
    assert!(
        context
            .evaluate_target("text/upper")
            .expect("one region")
            .passed
    );

    let error = context
        .evaluate_target("text/lower")
        .expect_err("the summed area exceeds one frame");

    assert_eq!(provider.calls(), 1);
    let outcome = error.outcome().expect("typed outcome");
    assert!(outcome.is(RecognitionPackCode::TextReadBudgetExceeded));
    assert_eq!(outcome.value("text_read_budget"), Some(&json!("area")));
    assert_eq!(outcome.value("count"), Some(&json!(640 * 240)));
    assert_eq!(outcome.value("limit_count"), Some(&json!(640 * 360)));
}

// Workflow #396 REC-8: a page gate's reads of the field and reading targets it keeps are reads
// of the page recognition, one per target.
#[test]
fn a_fifth_kept_target_a_page_gate_reads_is_refused_before_the_provider_is_called() {
    let provider =
        ScriptedOcr::new(|region| result(&[("ALPHA".to_owned(), region, Some(0.875))], None));
    let field = |index: i32| {
        ocr_target(
            &format!("text/{index}"),
            region_json(rect(index * 100, 0, 80, 40)),
            "exact",
            &["ALPHA"],
            None,
        )
    };
    let evaluator = evaluator(pack_json((0..5).map(field).collect()), provider.clone());
    let kept = (0..5)
        .map(|index| format!("text/{index}"))
        .collect::<HashSet<_>>();
    let scene = scene();
    let context = evaluator
        .scene_context(&scene)
        .with_remembered_targets(&kept);
    for index in 0..4 {
        assert!(
            context
                .evaluate_target(&format!("text/{index}"))
                .expect("within budget")
                .passed
        );
    }

    let error = context
        .evaluate_target("text/4")
        .expect_err("the fifth kept read exceeds the budget");

    assert_eq!(provider.calls(), 4);
    let outcome = error.outcome().expect("typed outcome");
    assert!(outcome.is(RecognitionPackCode::TextReadBudgetExceeded));
    assert_eq!(outcome.value("stage"), Some(&json!("text_read_request")));
    assert_eq!(outcome.value("text_read_budget"), Some(&json!("calls")));
    assert_eq!(outcome.value("count"), Some(&json!(4)));
    assert_eq!(outcome.value("limit_count"), Some(&json!(4)));
    assert_eq!(outcome.value("target_id"), Some(&json!("text/4")));
}

#[test]
fn a_sample_deadline_too_short_for_the_read_calls_no_ocr() {
    let provider = ScriptedOcr::blocks(vec![("ALPHA", rect(190, 240, 80, 20), Some(0.875))]);
    let evaluator = evaluator(
        pack_json(vec![locate_target(
            "text/entry",
            &["ALPHA"],
            "exact",
            detect(0),
        )]),
        provider.clone(),
    );
    let scene = scene();
    let context = evaluator
        .scene_context(&scene)
        .with_sample_deadline(Instant::now() + Duration::from_millis(100));

    context
        .evaluate_target("text/entry")
        .expect_err("a 1000 ms read does not fit a 100 ms deadline");

    assert_eq!(provider.calls(), 0);
}

#[test]
fn a_template_relative_region_locates_text_beside_its_anchor() {
    let icon = icon_pixels();
    let mut pixels = vec![0_u8; (FRAME_WIDTH * FRAME_HEIGHT * 3) as usize];
    for y in 0..ICON_HEIGHT {
        for x in 0..ICON_WIDTH {
            let source = ((y * ICON_WIDTH + x) * 3) as usize;
            let target = (((50 + y) * FRAME_WIDTH + 100 + x) * 3) as usize;
            pixels[target..target + 3].copy_from_slice(&icon[source..source + 3]);
        }
    }
    let scene = Scene::from_rgb8(FRAME_WIDTH, FRAME_HEIGHT, &pixels).expect("scene");
    let provider = ScriptedOcr::new(|region| {
        result(
            &[(
                "ALPHA".to_owned(),
                rect(region.x + 20, region.y + 10, 60, 20),
                Some(0.875),
            )],
            None,
        )
    });
    let pack = load_pack_from_json_str(
        &pack_json(vec![
            json!({"type": "template", "id": "anchor/icon", "template_path": "icon.png",
                   "region": {"x": 90, "y": 40, "width": 40, "height": 30}, "threshold": 0.9}),
            ocr_target(
                "text/beside",
                json!({"mode": "template_relative", "anchor_target_id": "anchor/icon",
                       "offset": {"x": 10, "y": 0}, "width": 200, "height": 40}),
                "exact",
                &["ALPHA"],
                Some(detect(0)),
            ),
        ])
        .to_string(),
    )
    .expect("pack");
    let evaluator = RecognitionEvaluator::with_vision_provider(
        pack,
        Arc::new(IconAsset(encode_png(ICON_WIDTH, ICON_HEIGHT, &icon))),
        provider.clone(),
    )
    .expect("evaluator");

    let evaluation = evaluator
        .evaluate_target(&scene, "text/beside")
        .expect("evaluated");

    assert!(evaluation.passed);
    assert_eq!(
        *provider.requests.lock().expect("request log"),
        [(
            rect(110, 50, 200, 40),
            OcrProviderBlocks::Detect {
                line_merge_gap_factor: Some(0)
            }
        )]
    );
    assert_eq!(located(&evaluation).rect, rect(130, 60, 60, 20));
}

#[test]
fn an_ocr_target_without_locate_keeps_its_request_and_output() {
    let provider = ScriptedOcr::blocks(vec![("ALPHA", rect(190, 240, 80, 20), Some(0.875))]);
    let evaluator = evaluator(
        pack_json(vec![ocr_target(
            "text/plain",
            region_json(list_region()),
            "exact",
            &["ALPHA"],
            None,
        )]),
        provider.clone(),
    );

    let evaluation = evaluate(&evaluator, "text/plain");

    assert!(evaluation.passed);
    assert_eq!(
        *provider.requests.lock().expect("request log"),
        [(list_region(), OcrProviderBlocks::Implicit)]
    );
    let value = serde_json::to_value(&evaluation).expect("serialize");
    let keys = |value: &Value| {
        let mut keys = value
            .as_object()
            .expect("object")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        keys.sort();
        keys
    };
    // No `failure` and no `located` member: the members it had before REC-1.
    assert_eq!(
        keys(&value),
        [
            "color", "id", "kind", "message", "nn", "ocr", "passed", "template"
        ]
    );
    assert_eq!(
        keys(&value["ocr"]),
        [
            "block_source_order",
            "blocks",
            "confidence",
            "match_mode",
            "matched_expected",
            "raw_text",
            "region",
            "text"
        ]
    );
}

#[test]
fn locate_declarations_are_refused_at_their_pointer() {
    use actingcommand_contract::ResourceDeclarationReason as Reason;
    let base = || {
        pack_json(vec![locate_target(
            "text/entry",
            &["ALPHA"],
            "exact",
            detect(0),
        )])
    };
    let full_frame = |locate: Value| {
        pack_json(vec![ocr_target(
            "text/entry",
            json!("full_frame"),
            "exact",
            &["ALPHA"],
            Some(locate),
        )])
    };
    let with_cells = |cells: Value| {
        let mut locate = detect(0);
        locate["cells"] = cells;
        pack_json(vec![locate_target(
            "text/entry",
            &["ALPHA"],
            "exact",
            locate,
        )])
    };
    let mut case_sensitive = base();
    case_sensitive["targets"][0]["case_sensitive"] = json!(true);
    let mut blank_expected = base();
    blank_expected["targets"][0]["expected"] = json!(["ALPHA", "  "]);
    let mut consensus = base();
    consensus["target_consensus"] = json!({"text/entry": {"samples": [{}], "k": 1}});
    let mut slot = base();
    slot["candidate_layouts"] = json!([{
        "id": "choices", "page_id": "neutral/list", "kind": "fixed_slots",
        "features": [{"name": "business_id", "value": "passed"}],
        "slots": [{"rect": {"x": 40, "y": 60, "width": 120, "height": 120},
                   "click": {"x": 40, "y": 60, "width": 120, "height": 120},
                   "targets": {"business_id": "text/entry"}}]
    }]);
    let mut feature = base();
    feature["candidate_layouts"] = json!([{
        "id": "choices", "page_id": "neutral/list", "kind": "fixed_slots",
        "features": [{"name": "business_id", "value": "passed",
                      "target": "text/entry", "offset": {"x": 0, "y": 0}}],
        "slots": [{"rect": {"x": 40, "y": 60, "width": 120, "height": 120},
                   "click": {"x": 40, "y": 60, "width": 120, "height": 120},
                   "targets": {}}]
    }]);
    let mut region_merge = base();
    region_merge["targets"][0]["locate"] =
        json!({"normalize": "identity_v1", "blocks": "region", "line_merge": "none"});
    let mut detect_without_merge = base();
    detect_without_merge["targets"][0]["locate"] =
        json!({"normalize": "identity_v1", "blocks": "detect"});
    let mut older_schema = base();
    older_schema["schema_version"] = json!("0.6");
    let mut too_many = cells();
    too_many["columns"] = json!(9);
    too_many["rows"] = json!(8);
    too_many["width"] = json!(1);
    too_many["height"] = json!(1);
    too_many["pitch_x"] = json!(1);
    too_many["pitch_y"] = json!(1);
    let mut overlapping = cells();
    overlapping["pitch_x"] = json!(100);
    let mut beyond = cells();
    beyond["columns"] = json!(5);

    let cases: Vec<(&str, Value, &str, Reason)> = vec![
        (
            "case_sensitive",
            case_sensitive,
            "/targets/0/case_sensitive",
            Reason::InvalidValue,
        ),
        (
            "blank expected",
            blank_expected,
            "/targets/0/expected/1",
            Reason::InvalidValue,
        ),
        (
            "target consensus",
            consensus,
            "/target_consensus/text~1entry",
            Reason::InvalidValue,
        ),
        (
            "candidate slot target",
            slot,
            "/candidate_layouts/0/slots/0/targets/business_id",
            Reason::InvalidValue,
        ),
        (
            "candidate feature target",
            feature,
            "/candidate_layouts/0/features/0/target",
            Reason::InvalidValue,
        ),
        (
            "region blocks on full_frame",
            full_frame(json!({"normalize": "identity_v1", "blocks": "region"})),
            "/targets/0/locate/blocks",
            Reason::InvalidValue,
        ),
        (
            "line_merge with region",
            region_merge,
            "/targets/0/locate/line_merge",
            Reason::UnconsumedField,
        ),
        (
            "detect without line_merge",
            detect_without_merge,
            "/targets/0/locate/line_merge",
            Reason::MissingField,
        ),
        (
            "cells beyond the region",
            with_cells(beyond),
            "/targets/0/locate/cells",
            Reason::InvalidValue,
        ),
        (
            "overlapping cells",
            with_cells(overlapping),
            "/targets/0/locate/cells/pitch_x",
            Reason::InvalidValue,
        ),
        (
            "more than 64 cells",
            with_cells(too_many),
            "/targets/0/locate/cells",
            Reason::InvalidValue,
        ),
        (
            "locate before schema 0.7",
            older_schema,
            "/targets/0/locate",
            Reason::UnconsumedField,
        ),
    ];
    for (case, pack, pointer, reason) in cases {
        let error = load_pack_from_json_str(&pack.to_string()).expect_err(case);
        let issue: &ResourceDeclarationIssue = error.declaration_issue().expect(case);
        assert_eq!(issue.field_path, pointer, "{case}");
        assert_eq!(issue.reason, reason, "{case}");
    }

    // A text anchor: the relative OCR anchor must stay a template.
    let anchored = pack_json(vec![
        locate_target("text/entry", &["ALPHA"], "exact", detect(0)),
        ocr_target(
            "text/beside",
            json!({"mode": "template_relative", "anchor_target_id": "text/entry",
                   "offset": {"x": 0, "y": 0}, "width": 40, "height": 20}),
            "exact",
            &["ALPHA"],
            None,
        ),
    ]);
    let pack = load_pack_from_json_str(&anchored.to_string()).expect("pack shape");
    let error = RecognitionEvaluator::with_vision_provider(
        pack,
        Arc::new(FsAssetResolver::new(PathBuf::new())),
        ScriptedOcr::blocks(Vec::new()),
    )
    .expect_err("a locate target is not an anchor");
    assert!(
        error
            .message()
            .contains("must name a static template target")
    );
}

const ICON_WIDTH: u32 = 8;
const ICON_HEIGHT: u32 = 6;

/// A patterned icon whose normalized correlation with a black frame is zero.
fn icon_pixels() -> Vec<u8> {
    let mut pixels = Vec::new();
    for y in 0..ICON_HEIGHT {
        for x in 0..ICON_WIDTH {
            pixels.extend_from_slice(&[
                ((x * 17 + y * 7) % 251 + 1) as u8,
                ((x * 11 + y * 19 + 23) % 239) as u8,
                ((x * 5 + y * 29 + 41) % 227) as u8,
            ]);
        }
    }
    pixels
}

#[derive(Debug)]
struct IconAsset(Vec<u8>);

impl AssetResolver for IconAsset {
    fn read_asset(&self, path: &str) -> RecognitionPackResult<Vec<u8>> {
        if path == "icon.png" {
            Ok(self.0.clone())
        } else {
            Err(RecognitionPackError::fatal("no such fixture asset"))
        }
    }
}

/// An uncompressed RGB PNG.
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
