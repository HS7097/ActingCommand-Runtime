// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #396 REC-2 logic tests: `unique` on template and locate OCR targets, its second
//! place and margin, the exact score map under a target, the search limit, and the evaluation
//! of targets without `unique`. Frames are synthetic neutral noise and color blocks; OCR is a
//! scripted provider.

use crate::*;
use actingcommand_contract::outcome::Category;
use serde_json::{Value, json};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ICON_WIDTH: u32 = 24;
const ICON_HEIGHT: u32 = 18;

/// A deterministic generator of neutral noise.
struct Noise(u64);

impl Noise {
    fn byte(&mut self) -> u8 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        // 1..=255: no pixel of a pattern is black.
        ((self.0 >> 33) % 255 + 1) as u8
    }

    /// An RGB pattern of `width` by `height` pixels.
    fn pattern(&mut self, width: u32, height: u32) -> Vec<u8> {
        (0..width * height * 3).map(|_| self.byte()).collect()
    }
}

/// An RGB frame, black unless painted.
struct Canvas {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Canvas {
    fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; (width * height * 3) as usize],
        }
    }

    fn paste(&mut self, x: u32, y: u32, width: u32, pattern: &[u8]) {
        for (row, line) in pattern.chunks((width * 3) as usize).enumerate() {
            let start = (((y + row as u32) * self.width + x) * 3) as usize;
            self.pixels[start..start + line.len()].copy_from_slice(line);
        }
    }

    fn fill(&mut self, x: u32, y: u32, width: u32, height: u32, color: [u8; 3]) {
        let pattern = color
            .iter()
            .copied()
            .cycle()
            .take((width * height * 3) as usize)
            .collect::<Vec<_>>();
        self.paste(x, y, width, &pattern);
    }

    fn scene(&self) -> Scene {
        Scene::from_rgb8(self.width, self.height, &self.pixels).expect("scene")
    }
}

#[derive(Debug, Default)]
struct Assets(HashMap<String, Vec<u8>>);

impl AssetResolver for Assets {
    fn read_asset(&self, path: &str) -> RecognitionPackResult<Vec<u8>> {
        self.0
            .get(path)
            .cloned()
            .ok_or_else(|| RecognitionPackError::fatal("no such fixture asset"))
    }
}

fn assets(templates: &[(&str, u32, u32, &[u8])]) -> Arc<Assets> {
    Arc::new(Assets(
        templates
            .iter()
            .map(|(path, width, height, pixels)| {
                ((*path).to_owned(), encode_png(*width, *height, pixels))
            })
            .collect(),
    ))
}

fn rect(x: i32, y: i32, width: i32, height: i32) -> PackRect {
    PackRect {
        x,
        y,
        width,
        height,
    }
}

fn template_target(id: &str, region: Value, unique: Option<u32>) -> Value {
    let mut target = json!({
        "type": "template", "id": id, "template_path": "icon.png", "region": region,
        "threshold": 0.9
    });
    if let Some(margin) = unique {
        target["unique"] = json!({"margin_milli": margin});
    }
    target
}

fn pack(width: u32, height: u32, metric: &str, targets: Vec<Value>) -> RecognitionPack {
    load_pack_from_json_str(
        &json!({
            "schema_version": "0.7",
            "coordinate_space": {"width": width, "height": height},
            "defaults": {"match_metric": metric},
            "targets": targets
        })
        .to_string(),
    )
    .expect("pack loads")
}

fn evaluator(pack: RecognitionPack, assets: Arc<Assets>) -> RecognitionEvaluator {
    RecognitionEvaluator::with_asset_resolver(pack, assets).expect("evaluator")
}

/// A clock that never moves: these evaluations check results, never the time limit.
fn frozen_clock() -> SearchClock {
    let now = Instant::now();
    SearchClock::new(move || now)
}

/// `floor(score * 1000)`: scores are compared in milli, rounded down.
fn milli(score: f32) -> u32 {
    (f64::from(score) * 1000.0).floor() as u32
}

fn failure(evaluation: &TargetEvaluation) -> &Outcome {
    evaluation.failure.as_ref().expect("a typed failure")
}

fn uniqueness(evaluation: &TargetEvaluation) -> TargetUniquenessEvidence {
    evaluation.uniqueness.expect("uniqueness evidence")
}

#[test]
fn two_identical_blocks_apart_make_a_unique_template_fail_under_its_code() {
    let icon = Noise(0x0396_0101).pattern(ICON_WIDTH, ICON_HEIGHT);
    let assets = assets(&[("icon.png", ICON_WIDTH, ICON_HEIGHT, icon.as_slice())]);
    let evaluator = evaluator(
        pack(
            320,
            240,
            "ccorr_normed",
            vec![template_target(
                "icon/unique",
                json!("full_frame"),
                Some(50),
            )],
        ),
        assets,
    );
    let mut canvas = Canvas::new(320, 240);
    canvas.paste(40, 30, ICON_WIDTH, &icon);

    let once = evaluator
        .evaluate_target(&canvas.scene(), "icon/unique")
        .expect("one block");
    assert!(once.passed);
    assert_eq!(once.failure, None);
    let evidence = uniqueness(&once);
    assert_eq!(evidence.second_score_milli, None);
    assert_eq!(evidence.second_rect, None);
    assert_eq!(evidence.margin_milli, 50);

    canvas.paste(200, 150, ICON_WIDTH, &icon);
    let twice = evaluator
        .evaluate_target(&canvas.scene(), "icon/unique")
        .expect("two blocks");

    assert!(!twice.passed);
    let template = twice.template.expect("template evidence");
    assert_eq!((template.x, template.y), (40, 30), "the first in (y, x)");
    let failure = failure(&twice);
    assert!(failure.is(RecognitionPackCode::TargetMatchNotUnique));
    assert_eq!(failure.category(), Category::Error);
    assert_eq!(failure.value("target_id"), Some(&json!("icon/unique")));
    assert_eq!(failure.value("target_kind"), Some(&json!("template")));
    assert_eq!(failure.value("margin_milli"), Some(&json!(50)));
    assert_eq!(failure.value("stage"), Some(&json!("target_match_unique")));
    let evidence = uniqueness(&twice);
    assert_eq!(evidence.score_milli, milli(template.score));
    assert_eq!(evidence.second_score_milli, Some(evidence.score_milli));
    assert_eq!(
        failure.value("score_milli"),
        Some(&json!(evidence.score_milli))
    );
    assert_eq!(
        failure.value("second_score_milli"),
        Some(&json!(evidence.score_milli))
    );
    assert_eq!(
        evidence.second_rect,
        Some(rect(200, 150, ICON_WIDTH as i32, ICON_HEIGHT as i32))
    );
}

#[test]
fn a_second_place_exactly_at_the_margin_fails_and_one_milli_below_it_passes() {
    let icon = Noise(0x0396_0102).pattern(ICON_WIDTH, ICON_HEIGHT);
    // A second block that differs from the icon in one pixel of every eight.
    let mut altered = icon.clone();
    for pixel in altered.chunks_mut(3).step_by(8) {
        for channel in pixel {
            *channel = 255 - *channel;
        }
    }
    let mut canvas = Canvas::new(320, 240);
    canvas.paste(40, 30, ICON_WIDTH, &icon);
    canvas.paste(200, 150, ICON_WIDTH, &altered);
    let scene = canvas.scene();
    let png = encode_png(ICON_WIDTH, ICON_HEIGHT, &icon);
    let score_at = |x: i32, y: i32| {
        scene
            .match_template_with_metric(
                &png,
                Some(recognition::Rect {
                    x,
                    y,
                    width: ICON_WIDTH as i32,
                    height: ICON_HEIGHT as i32,
                }),
                recognition::MatchMetric::CorrelationCoefficientNormalized,
            )
            .expect("the score at one position")
            .score
    };
    let best = milli(score_at(40, 30));
    let second = milli(score_at(200, 150));
    assert!(best >= second + 2, "best {best}, second {second}");
    let unique_with = |margin: u32| {
        evaluator(
            pack(
                320,
                240,
                "ccoeff_normed",
                vec![template_target(
                    "icon/unique",
                    json!("full_frame"),
                    Some(margin),
                )],
            ),
            assets(&[("icon.png", ICON_WIDTH, ICON_HEIGHT, icon.as_slice())]),
        )
        .evaluate_target(&scene, "icon/unique")
        .expect("evaluated")
    };

    let at_margin = unique_with(best - second);
    assert!(!at_margin.passed, "a second place exactly at the margin");
    assert!(failure(&at_margin).is(RecognitionPackCode::TargetMatchNotUnique));
    assert_eq!(uniqueness(&at_margin).score_milli, best);
    assert_eq!(uniqueness(&at_margin).second_score_milli, Some(second));

    let below_margin = unique_with(best - second - 1);
    assert!(
        below_margin.passed,
        "a second place 1 milli below the margin"
    );
    assert_eq!(below_margin.failure, None);
    assert_eq!(uniqueness(&below_margin).second_score_milli, None);
}

#[test]
fn neighbours_overlapping_the_best_match_are_never_a_second_place() {
    // A smooth icon: positions one pixel off still score close to the best.
    let icon = (0..ICON_HEIGHT)
        .flat_map(|y| (0..ICON_WIDTH).map(move |x| (40 + 4 * x + 3 * y) as u8))
        .flat_map(|value| [value; 3])
        .collect::<Vec<_>>();
    let mut canvas = Canvas::new(320, 240);
    canvas.paste(100, 80, ICON_WIDTH, &icon);
    let scene = canvas.scene();
    let png = encode_png(ICON_WIDTH, ICON_HEIGHT, &icon);
    let neighbour = scene
        .match_template_with_metric(
            &png,
            Some(recognition::Rect {
                x: 101,
                y: 80,
                width: ICON_WIDTH as i32,
                height: ICON_HEIGHT as i32,
            }),
            recognition::MatchMetric::CrossCorrelationNormalized,
        )
        .expect("the neighbour's score")
        .score;
    let evaluator = evaluator(
        pack(
            320,
            240,
            "ccorr_normed",
            vec![template_target(
                "icon/unique",
                json!("full_frame"),
                Some(100),
            )],
        ),
        assets(&[("icon.png", ICON_WIDTH, ICON_HEIGHT, icon.as_slice())]),
    );

    let evaluation = evaluator
        .evaluate_target(&scene, "icon/unique")
        .expect("evaluated");

    assert!(evaluation.passed);
    let evidence = uniqueness(&evaluation);
    assert!(
        milli(neighbour) + 100 >= evidence.score_milli,
        "the neighbour scores within the margin: {} of {}",
        milli(neighbour),
        evidence.score_milli
    );
    assert_eq!(evidence.second_score_milli, None);
}

#[test]
fn a_second_place_whose_relative_color_fails_is_not_counted() {
    let icon = Noise(0x0396_0104).pattern(ICON_WIDTH, ICON_HEIGHT);
    let target = || {
        let mut target = template_target("icon/unique", json!("full_frame"), Some(1000));
        target["color_check"] = json!({
            "region": {"mode": "template_relative", "anchor_target_id": "icon/unique",
                       "offset": {"x": 30, "y": 0}, "width": 6, "height": 6},
            "expected": [255, 0, 0], "max_distance": 10.0
        });
        target
    };
    let evaluate = |second_patch: [u8; 3]| {
        let mut canvas = Canvas::new(320, 240);
        canvas.paste(40, 30, ICON_WIDTH, &icon);
        canvas.fill(70, 30, 6, 6, [255, 0, 0]);
        canvas.paste(200, 150, ICON_WIDTH, &icon);
        canvas.fill(230, 150, 6, 6, second_patch);
        evaluator(
            pack(320, 240, "ccorr_normed", vec![target()]),
            assets(&[("icon.png", ICON_WIDTH, ICON_HEIGHT, icon.as_slice())]),
        )
        .evaluate_target(&canvas.scene(), "icon/unique")
        .expect("evaluated")
    };

    let wrong_color = evaluate([0, 0, 255]);
    assert!(wrong_color.passed, "the second block's color fails");
    assert_eq!(uniqueness(&wrong_color).second_score_milli, None);
    let template = wrong_color.template.expect("template evidence");
    assert_eq!((template.x, template.y), (40, 30));

    let same_color = evaluate([255, 0, 0]);
    assert!(!same_color.passed, "the second block's color holds too");
    assert!(failure(&same_color).is(RecognitionPackCode::TargetMatchNotUnique));
    assert_eq!(
        uniqueness(&same_color).second_rect,
        Some(rect(200, 150, ICON_WIDTH as i32, ICON_HEIGHT as i32))
    );
}

#[test]
fn a_full_frame_unique_template_is_admitted_and_found_once_but_not_twice() {
    for size in [32_u32, 64] {
        let icon = Noise(0x0396_0105 + u64::from(size)).pattern(size, size);
        let full_frame = || {
            evaluator(
                pack(
                    1280,
                    720,
                    "ccorr_normed",
                    vec![template_target(
                        "icon/unique",
                        json!("full_frame"),
                        Some(50),
                    )],
                ),
                assets(&[("icon.png", size, size, icon.as_slice())]),
            )
            .with_search_clock(frozen_clock())
        };
        let mut canvas = Canvas::new(1280, 720);
        canvas.paste(700, 400, size, &icon);

        let once = full_frame()
            .evaluate_target(&canvas.scene(), "icon/unique")
            .expect("one block in the frame");
        assert!(once.passed, "{size} px, one block");
        let template = once.template.expect("template evidence");
        assert_eq!((template.x, template.y), (700, 400));

        canvas.paste(100, 120, size, &icon);
        let twice = full_frame()
            .evaluate_target(&canvas.scene(), "icon/unique")
            .expect("two blocks in the frame");
        assert!(!twice.passed, "{size} px, two blocks");
        assert!(failure(&twice).is(RecognitionPackCode::TargetMatchNotUnique));
        assert_eq!(
            uniqueness(&twice).second_rect,
            Some(rect(700, 400, size as i32, size as i32)),
            "{size} px: the first in (y, x) is the best"
        );
    }
}

#[test]
fn a_unique_search_past_its_time_limit_fails_as_incomplete_and_never_passes() {
    let icon = Noise(0x0396_0106).pattern(ICON_WIDTH, ICON_HEIGHT);
    let mut canvas = Canvas::new(320, 240);
    canvas.paste(40, 30, ICON_WIDTH, &icon);
    // Every reading of the clock is 6 s after the previous one.
    let base = Instant::now();
    let readings = Arc::new(AtomicU64::new(0));
    let clock = SearchClock::new(move || {
        base + Duration::from_secs(6 * readings.fetch_add(1, Ordering::SeqCst))
    });
    let evaluator = evaluator(
        pack(
            320,
            240,
            "ccorr_normed",
            vec![template_target(
                "icon/unique",
                json!("full_frame"),
                Some(50),
            )],
        ),
        assets(&[("icon.png", ICON_WIDTH, ICON_HEIGHT, icon.as_slice())]),
    )
    .with_search_clock(clock);

    let error = evaluator
        .evaluate_target(&canvas.scene(), "icon/unique")
        .expect_err("the search did not finish in time");

    assert!(error.timing().is_some(), "the deadline observation");
    let outcome = error.outcome().expect("typed outcome");
    assert!(outcome.is(RecognitionPackCode::TargetMatchSearchIncomplete));
    assert_eq!(outcome.category(), Category::Error);
    assert_eq!(outcome.value("target_id"), Some(&json!("icon/unique")));
    assert_eq!(outcome.value("timeout_ms"), Some(&json!(5000)));
    assert_eq!(outcome.value("stage"), Some(&json!("target_match_unique")));
}

type Respond = dyn Fn(PackRect) -> OcrProviderResult + Send + Sync;

/// Answers every OCR request from its region and records each call.
struct ScriptedOcr {
    respond: Box<Respond>,
    regions: Mutex<Vec<PackRect>>,
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
            regions: Mutex::new(Vec::new()),
        })
    }

    fn blocks(blocks: Vec<(&str, PackRect, Option<f32>)>) -> Arc<Self> {
        let blocks = blocks
            .into_iter()
            .map(|(text, rect, confidence)| (text.to_owned(), rect, confidence))
            .collect::<Vec<_>>();
        Self::new(move |_| ocr_result(&blocks, None))
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
        self.regions
            .lock()
            .expect("region log")
            .push(request.region);
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

fn ocr_result(
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

fn ocr_target(id: &str, region: Value, locate: bool, unique: Option<u32>) -> Value {
    let mut target = json!({
        "type": "ocr", "id": id, "region": region, "languages": ["en"], "timeout_ms": 1000,
        "match_mode": "exact", "expected": ["ALPHA"], "case_sensitive": false,
        "minimum_confidence": 0.8, "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256
    });
    if locate {
        target["locate"] = json!({"normalize": "identity_v1", "blocks": "detect",
                                  "line_merge": {"gap_factor": 0}});
    }
    if let Some(margin) = unique {
        target["unique"] = json!({"margin_milli": margin});
    }
    target
}

/// A locate target over the list region with `unique`, read by `provider`.
fn located_text(provider: Arc<ScriptedOcr>, margin: u32) -> TargetEvaluation {
    RecognitionEvaluator::with_vision_provider(
        pack(
            640,
            360,
            "ccorr_normed",
            vec![ocr_target(
                "text/entry",
                json!({"x": 40, "y": 60, "width": 560, "height": 280}),
                true,
                Some(margin),
            )],
        ),
        Arc::new(Assets::default()),
        provider,
    )
    .expect("evaluator")
    .evaluate_target(&Canvas::new(640, 360).scene(), "text/entry")
    .expect("evaluated")
}

#[test]
fn two_matching_text_blocks_make_a_unique_locate_target_fail_under_its_code() {
    let twice = located_text(
        ScriptedOcr::blocks(vec![
            ("ALPHA", rect(60, 80, 60, 20), Some(0.875)),
            ("ALPHA", rect(330, 240, 60, 20), Some(0.75)),
        ]),
        1000,
    );

    assert!(!twice.passed);
    assert_eq!(twice.ocr.as_ref().and_then(|ocr| ocr.located), None);
    let failure = failure(&twice);
    assert!(failure.is(RecognitionPackCode::TargetMatchNotUnique));
    assert_eq!(failure.category(), Category::Error);
    assert_eq!(failure.value("target_id"), Some(&json!("text/entry")));
    assert_eq!(failure.value("target_kind"), Some(&json!("ocr")));
    assert_eq!(failure.value("score_milli"), Some(&json!(875)));
    assert_eq!(failure.value("second_score_milli"), Some(&json!(750)));
    assert_eq!(failure.value("margin_milli"), Some(&json!(1000)));
    assert_eq!(failure.value("stage"), Some(&json!("target_match_unique")));
    assert_eq!(
        uniqueness(&twice),
        TargetUniquenessEvidence {
            score_milli: 875,
            second_score_milli: Some(750),
            margin_milli: 1000,
            second_rect: Some(rect(330, 240, 60, 20)),
        }
    );

    let once = located_text(
        ScriptedOcr::blocks(vec![
            ("ALPHA", rect(60, 80, 60, 20), Some(0.875)),
            ("GAMMA", rect(330, 240, 60, 20), Some(0.875)),
        ]),
        1000,
    );
    assert!(once.passed);
    assert_eq!(uniqueness(&once).second_score_milli, None);
}

#[test]
fn a_weak_text_match_within_the_margin_is_a_second_place() {
    // The second block's confidence is below minimum_confidence (0.8): a weak hit.
    let blocks = || {
        ScriptedOcr::blocks(vec![
            ("ALPHA", rect(60, 80, 60, 20), Some(0.875)),
            ("ALPHA", rect(330, 240, 60, 20), Some(0.5)),
        ])
    };

    let within = located_text(blocks(), 400);
    assert!(!within.passed, "500 is within 875 - 400");
    assert!(failure(&within).is(RecognitionPackCode::TargetMatchNotUnique));
    assert_eq!(uniqueness(&within).second_score_milli, Some(500));

    let beyond = located_text(blocks(), 300);
    assert!(beyond.passed, "500 is below 875 - 300");
    assert_eq!(uniqueness(&beyond).second_score_milli, None);
}

#[test]
fn a_hit_among_truncated_boxes_cannot_conclude_a_unique_target() {
    let provider = ScriptedOcr::new(|_| {
        ocr_result(
            &[("ALPHA".to_owned(), rect(190, 240, 80, 20), Some(0.875))],
            Some(OcrProviderDetection {
                detected: 65,
                limit: 64,
            }),
        )
    });
    let error = RecognitionEvaluator::with_vision_provider(
        pack(
            640,
            360,
            "ccorr_normed",
            vec![ocr_target(
                "text/entry",
                json!({"x": 40, "y": 60, "width": 560, "height": 280}),
                true,
                Some(1000),
            )],
        ),
        Arc::new(Assets::default()),
        provider,
    )
    .expect("evaluator")
    .evaluate_target(&Canvas::new(640, 360).scene(), "text/entry")
    .expect_err("dropped boxes could hold a second place");

    assert_eq!(error.code(), RecognitionPackErrorCode::TextReadRefused);
    let outcome = error.outcome().expect("typed outcome");
    assert!(outcome.is(RecognitionPackCode::TextReadBlocksTruncated));
    assert_eq!(outcome.category(), Category::Error);
    assert_eq!(outcome.value("target_id"), Some(&json!("text/entry")));
    assert_eq!(outcome.value("count"), Some(&json!(65)));
    assert_eq!(outcome.value("limit_count"), Some(&json!(64)));
    assert_eq!(outcome.value("stage"), Some(&json!("text_locate_evaluate")));
}

#[test]
fn a_target_without_unique_keeps_its_evaluation_bytes() {
    let icon = Noise(0x0396_0110).pattern(ICON_WIDTH, ICON_HEIGHT);
    let mut canvas = Canvas::new(320, 240);
    canvas.paste(40, 30, ICON_WIDTH, &icon);
    canvas.fill(70, 30, 6, 6, [255, 0, 0]);
    let scene = canvas.scene();
    let png = encode_png(ICON_WIDTH, ICON_HEIGHT, &icon);
    let region = rect(20, 10, 120, 90);
    let number = |value: f32| serde_json::to_string(&value).expect("number");

    for (metric, primitive) in [
        (
            "ccorr_normed",
            recognition::MatchMetric::CrossCorrelationNormalized,
        ),
        (
            "ccoeff_normed",
            recognition::MatchMetric::CorrelationCoefficientNormalized,
        ),
    ] {
        let evaluation = evaluator(
            pack(
                320,
                240,
                metric,
                vec![template_target(
                    "icon/plain",
                    json!({"x": 20, "y": 10, "width": 120, "height": 90}),
                    None,
                )],
            ),
            assets(&[("icon.png", ICON_WIDTH, ICON_HEIGHT, icon.as_slice())]),
        )
        .evaluate_target(&scene, "icon/plain")
        .expect("evaluated");
        let matched = scene
            .match_template_with_metric(&png, Some(region.into()), primitive)
            .expect("the search primitive");
        assert_eq!(
            serde_json::to_string(&evaluation).expect("serialize"),
            format!(
                r#"{{"id":"icon/plain","kind":"template","passed":true,"template":{{"x":{},"y":{},"width":{},"height":{},"raw_score":{},"score":{},"threshold":{}}},"color":null,"ocr":null,"nn":null,"message":"template passed"}}"#,
                matched.x,
                matched.y,
                matched.width,
                matched.height,
                number(matched.raw_score),
                number(matched.score),
                number(0.9)
            ),
            "{metric}"
        );
    }

    let mut relative = template_target(
        "icon/plain",
        json!({"x": 20, "y": 10, "width": 120, "height": 90}),
        None,
    );
    relative["color_check"] = json!({
        "region": {"mode": "template_relative", "anchor_target_id": "icon/plain",
                   "offset": {"x": 30, "y": 0}, "width": 6, "height": 6},
        "expected": [255, 0, 0], "max_distance": 10.0
    });
    let evaluation = evaluator(
        pack(320, 240, "ccorr_normed", vec![relative]),
        assets(&[("icon.png", ICON_WIDTH, ICON_HEIGHT, icon.as_slice())]),
    )
    .evaluate_target(&scene, "icon/plain")
    .expect("evaluated");
    assert!(evaluation.passed);
    let value = serde_json::to_value(&evaluation).expect("serialize");
    let mut keys = value
        .as_object()
        .expect("object")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    assert_eq!(
        keys,
        [
            "color", "id", "kind", "message", "nn", "ocr", "passed", "template"
        ]
    );
}

#[test]
fn a_unique_anchor_anchors_its_ocr_region_only_while_it_is_unique() {
    let icon = Noise(0x0396_0111).pattern(ICON_WIDTH, ICON_HEIGHT);
    let read = |copies: &[(u32, u32)]| {
        let mut canvas = Canvas::new(320, 240);
        for (x, y) in copies {
            canvas.paste(*x, *y, ICON_WIDTH, &icon);
        }
        let provider = ScriptedOcr::new(|region| {
            ocr_result(&[("ALPHA".to_owned(), region, Some(0.875))], None)
        });
        let result = RecognitionEvaluator::with_vision_provider(
            pack(
                320,
                240,
                "ccorr_normed",
                vec![
                    template_target("icon/anchor", json!("full_frame"), Some(50)),
                    ocr_target(
                        "text/beside",
                        json!({"mode": "template_relative", "anchor_target_id": "icon/anchor",
                               "offset": {"x": 30, "y": 0}, "width": 80, "height": 20}),
                        false,
                        None,
                    ),
                ],
            ),
            assets(&[("icon.png", ICON_WIDTH, ICON_HEIGHT, icon.as_slice())]),
            provider.clone(),
        )
        .expect("evaluator")
        .evaluate_target(&canvas.scene(), "text/beside");
        let regions = provider.regions.lock().expect("region log").clone();
        (result, regions)
    };

    let (result, regions) = read(&[(40, 30)]);
    assert!(result.expect("one anchor").passed);
    assert_eq!(regions, [rect(70, 30, 80, 20)]);

    // Workflow #390 R-10: the OCR target whose anchor did not pass did not pass, with its
    // region evidence.
    let (result, regions) = read(&[(40, 30), (200, 150)]);
    let evaluation = result.expect("two anchors anchor nothing");
    assert!(!evaluation.passed);
    assert!(
        evaluation
            .failure
            .as_ref()
            .is_some_and(|failure| failure.is(RecognitionPackCode::OcrRegionUnresolved))
    );
    let region = &evaluation.ocr.as_ref().expect("region evidence").region;
    assert_eq!(
        region.unresolved,
        Some(OcrRegionUnresolvedReason::AnchorNotMatched)
    );
    assert_eq!(
        region.anchor_match.as_ref().map(|anchor| anchor.passed),
        Some(false)
    );
    assert!(regions.is_empty(), "no OCR call");
}

/// Workflow #308 SEL-L1 merge: a candidate layout feature read at an offset and a
/// `repeated_anchor` anchor check no second place, so a template that declares `unique` is
/// refused in either role when the evaluator is built, at its pointer.
#[test]
fn a_unique_template_is_refused_as_an_offset_feature_or_a_repeated_anchor() {
    let icon = Noise(0x0396_0112).pattern(ICON_WIDTH, ICON_HEIGHT);
    let build = |anchor_unique: Option<u32>, feature_unique: Option<u32>| {
        let pack = json!({
            "schema_version": "0.7",
            "coordinate_space": {"width": 320, "height": 240},
            "defaults": {"match_metric": "ccoeff_normed"},
            "targets": [
                template_target("icon/row", json!("full_frame"), anchor_unique),
                template_target(
                    "icon/row_state",
                    json!({"x": 0, "y": 0, "width": 30, "height": 20}),
                    feature_unique,
                ),
            ],
            "candidate_layouts": [{
                "id": "layout/rows", "page_id": "list_page", "kind": "repeated_anchor",
                "anchor": "icon/row", "max_instances": 4, "order": "top_to_bottom",
                "instance_rect": {"x": -2, "y": -2, "width": 80, "height": 24},
                "click": {"x": 0, "y": 0, "width": 20, "height": 18},
                "features": [{"name": "state", "value": "passed", "target": "icon/row_state",
                              "offset": {"x": 30, "y": 0}}]
            }]
        });
        RecognitionEvaluator::with_asset_resolver(
            load_pack_from_json_str(&pack.to_string()).expect("pack loads"),
            assets(&[("icon.png", ICON_WIDTH, ICON_HEIGHT, icon.as_slice())]),
        )
    };

    build(None, None).expect("the layout without unique is admitted");
    for (anchor_unique, feature_unique, pointer) in [
        (
            Some(50),
            None,
            "/candidate_layouts/0/anchor: template 'icon/row' declares unique",
        ),
        (
            None,
            Some(50),
            "/candidate_layouts/0/features/0/target: template 'icon/row_state' declares unique",
        ),
    ] {
        let error = build(anchor_unique, feature_unique).expect_err(pointer);
        assert!(
            error.message().contains(pointer),
            "expected '{pointer}' in '{}'",
            error.message()
        );
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
