// SPDX-License-Identifier: AGPL-3.0-only

// #463 RG-1: `recognize` answers the box of a text it located. For an OCR target with `locate`
// that passed, the answer's `matched_rect` (and its evaluation's) is the box a `target` or
// `offset` click that follows the text starts from, taken from the one function the run takes it
// from (`located_text_box`: the declared cell holding the found block, else the block); a target
// that did not pass, and an OCR target without `locate`, name no box, and a template target is
// answered as before. `ReadonlyRecognitionEngine::recognize` is the function the Lab's offline and
// captured `recognize` calls.
//
// The scene is synthetic: 160x90 pixels of flat gray, a 16x16 icon of neutral noise at (20, 8),
// and, when the names are shown, one marker pixel inside the names' region. The scripted text
// reader reads the blocks GAMMA and ALPHA at fixed places of the region it is asked for when the
// marker is shown, and nothing otherwise, as the kernel's tests of a located text read theirs. No
// model, no device, no clock.

use crate::contained_task::located_text_box;
use crate::{
    ReadonlyRecognitionEngine, RecognizeEvaluatedResponse, RecognizeResponse, RectResponse,
};
use actingcommand_device::PixelFormat;
use actingcommand_recognition::{Scene, ScenePixelFormat};
use actingcommand_recognition_pack::{
    AssetResolver, NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, PackRect, RecognitionEvaluator, RecognitionPackError,
    RecognitionPackResult, VisionProvider, VisionProviderError, VisionProviderErrorCode,
    load_pack_from_json_str,
};
use serde_json::json;
use std::sync::Arc;

const WIDTH: u32 = 160;
const HEIGHT: u32 = 90;
const BACKGROUND: u8 = 60;
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

// The template target and its icon.
const ICON: &str = "ui/icon";
const ICON_ASSET: &str = "assets/icon.png";
const ICON_SIZE: u32 = 16;
const ICON_X: u32 = 20;
const ICON_Y: u32 = 8;

// The located-text targets: one without cells, one whose region is a row of four cells.
const ENTRY: &str = "text/entry";
const CELL: &str = "text/cell";
// The OCR target without `locate`, on the same region.
const PLAIN: &str = "text/plain";

// The region the three text targets read.
const NAMES: PackRect = PackRect {
    x: 8,
    y: 40,
    width: 144,
    height: 40,
};
// The marker pixel, inside the region, and its gray value: the names are shown when it is set.
const MARKER: u8 = 211;
const MARKER_X: u32 = 10;
const MARKER_Y: u32 = 42;
const CONFIDENCE: f32 = 0.875;

// Reads GAMMA and ALPHA at fixed places of the requested region when the frame shows the marker,
// and no block otherwise.
#[derive(Debug)]
struct NameReader;

impl VisionProvider for NameReader {
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
        let region = request.region;
        let marker = ((MARKER_Y * request.frame.width + MARKER_X) * 3) as usize;
        let blocks = if request.frame.rgb8_pixels.get(marker) == Some(&MARKER) {
            vec![
                block("GAMMA", region.x + 4, region.y + 10, 24),
                block("ALPHA", region.x + 76, region.y + 10, 28),
            ]
        } else {
            Vec::new()
        };
        let text = blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            confidence: (!blocks.is_empty()).then_some(CONFIDENCE),
            text,
            blocks,
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

// One block of `text`, 12 pixels high.
fn block(text: &str, x: i32, y: i32, width: i32) -> OcrProviderTextBlock {
    OcrProviderTextBlock {
        text: text.to_owned(),
        rect: PackRect {
            x,
            y,
            width,
            height: 12,
        },
        confidence: Some(CONFIDENCE),
    }
}

// The icon's PNG, the one asset of the pack.
#[derive(Debug)]
struct IconAsset(Vec<u8>);

impl AssetResolver for IconAsset {
    fn read_asset(&self, path: &str) -> RecognitionPackResult<Vec<u8>> {
        if path == ICON_ASSET {
            return Ok(self.0.clone());
        }
        Err(RecognitionPackError::fatal(format!(
            "this fixture has no asset '{path}'"
        )))
    }
}

// The icon's gray values, row-major: deterministic neutral noise.
fn icon() -> Vec<u8> {
    let mut state: u64 = 0x0463_0001;
    (0..ICON_SIZE * ICON_SIZE)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            1 + ((state >> 33) % 200) as u8
        })
        .collect()
}

// Gray values drawn with equal red, green and blue.
fn rgb(values: &[u8]) -> Vec<u8> {
    values.iter().flat_map(|value| [*value; 3]).collect()
}

// The scene: the icon at (20, 8) and, with `names`, the marker inside the names' region.
fn scene(names: bool) -> Scene {
    let mut values = vec![BACKGROUND; (WIDTH * HEIGHT) as usize];
    let icon = icon();
    for row in 0..ICON_SIZE {
        for column in 0..ICON_SIZE {
            values[((ICON_Y + row) * WIDTH + ICON_X + column) as usize] =
                icon[(row * ICON_SIZE + column) as usize];
        }
    }
    if names {
        values[(MARKER_Y * WIDTH + MARKER_X) as usize] = MARKER;
    }
    Scene::from_pixels(WIDTH, HEIGHT, &rgb(&values), ScenePixelFormat::Rgb8).expect("scene")
}

fn text_target(id: &str, locate: Option<serde_json::Value>, match_mode: &str) -> serde_json::Value {
    let mut target = json!({
        "type": "ocr", "id": id,
        "region": {"x": NAMES.x, "y": NAMES.y, "width": NAMES.width, "height": NAMES.height},
        "languages": ["en"], "timeout_ms": 1000, "match_mode": match_mode, "expected": ["ALPHA"],
        "case_sensitive": false, "minimum_confidence": 0.8,
        "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256
    });
    if let Some(locate) = locate {
        target["locate"] = locate;
    }
    target
}

// The pack: the icon's template, the two located-text targets and the OCR target without
// `locate`, with the scripted reader and the icon's PNG.
fn evaluator() -> RecognitionEvaluator {
    let detect = json!({"normalize": "identity_v1", "blocks": "detect", "line_merge": "none"});
    let mut in_cells = detect.clone();
    in_cells["cells"] = json!({"x": 0, "y": 0, "width": 36, "height": 40, "pitch_x": 36,
                               "pitch_y": 40, "columns": 4, "rows": 1});
    let pack = load_pack_from_json_str(
        &json!({
            "schema_version": "0.7",
            "game": "neutral",
            "server": "test",
            "coordinate_space": {"width": WIDTH, "height": HEIGHT},
            "defaults": {"match_metric": "ccoeff_normed"},
            "targets": [
                {"type": "template", "id": ICON, "template_path": ICON_ASSET,
                 "region": "full_frame", "threshold": 0.9},
                text_target(ENTRY, Some(detect), "exact"),
                text_target(CELL, Some(in_cells), "exact"),
                text_target(PLAIN, None, "contains"),
            ]
        })
        .to_string(),
    )
    .expect("recognition pack");
    let png = actingcommand_device::capture::encode_png_fast(
        ICON_SIZE,
        ICON_SIZE,
        &rgb(&icon()),
        PixelFormat::Rgb8,
    )
    .expect("icon PNG");
    RecognitionEvaluator::with_vision_provider(pack, Arc::new(IconAsset(png)), Arc::new(NameReader))
        .expect("recognition evaluator")
}

fn evaluated(response: RecognizeResponse) -> RecognizeEvaluatedResponse {
    match response {
        RecognizeResponse::Evaluated(answer) => *answer,
        RecognizeResponse::ClickOnly(_) => panic!("an evaluated target is answered evaluated"),
    }
}

fn pack_rect(rect: Option<RectResponse>) -> Option<PackRect> {
    rect.map(|rect| PackRect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    })
}

// (1) On a scene that shows the names, each located-text target passes and `recognize` answers
// as `matched_rect`, and as its evaluation's, the box the run's function gives for the same scene
// and target: for the target with cells the cell holding ALPHA, for the other ALPHA's block.
#[test]
fn rg1_a_found_text_answers_the_box_the_run_starts_its_click_from() {
    let scene = scene(true);
    let evaluator = evaluator();
    let engine = ReadonlyRecognitionEngine::new(evaluator.clone(), Vec::new());
    for target in [ENTRY, CELL] {
        let answer = evaluated(engine.recognize(target, Some(&scene)).expect("recognize"));
        let evaluation = evaluator
            .evaluate_target(&scene, target)
            .expect("the target's evaluation");
        let located = evaluation
            .ocr
            .as_ref()
            .and_then(|ocr| ocr.located)
            .expect("the found text");
        assert_eq!(located.cell_rect.is_some(), target == CELL, "{target}");
        let found = located_text_box(&evaluation).expect("the box of the found text");
        assert!(answer.passed, "{target}");
        assert_eq!(pack_rect(answer.matched_rect), Some(found), "{target}");
        assert_eq!(
            pack_rect(answer.evaluation.matched_rect),
            Some(found),
            "{target}"
        );
    }
}

// (2) The same targets on a scene that does not show the names: not passed, no box.
#[test]
fn rg1_on_a_scene_without_the_text_the_target_does_not_pass_and_names_no_box() {
    let engine = ReadonlyRecognitionEngine::new(evaluator(), Vec::new());
    let scene = scene(false);
    for target in [ENTRY, CELL] {
        let answer = evaluated(engine.recognize(target, Some(&scene)).expect("recognize"));
        assert!(!answer.passed, "{target}");
        assert!(answer.matched_rect.is_none(), "{target}");
        assert!(answer.evaluation.matched_rect.is_none(), "{target}");
    }
}

// (3) An OCR target without `locate` passes on the names' scene and names no box, as before.
#[test]
fn rg1_an_ocr_target_without_locate_passes_and_names_no_box_as_before() {
    let engine = ReadonlyRecognitionEngine::new(evaluator(), Vec::new());
    let answer = evaluated(
        engine
            .recognize(PLAIN, Some(&scene(true)))
            .expect("recognize"),
    );
    assert!(answer.passed);
    assert!(answer.matched_rect.is_none());
    assert!(answer.evaluation.matched_rect.is_none());
}

// (4) A template target is answered as it was before this slice, member for member: its
// matched rectangle is the template's match, in the answer and in its evaluation.
#[test]
fn rg1_a_template_target_is_answered_byte_for_byte_as_before() {
    let scene = scene(true);
    let evaluator = evaluator();
    let engine = ReadonlyRecognitionEngine::new(evaluator.clone(), Vec::new());
    let answer = engine.recognize(ICON, Some(&scene)).expect("recognize");
    let evaluation = evaluator
        .evaluate_target(&scene, ICON)
        .expect("the template's evaluation");
    assert!(evaluation.passed);
    let template = evaluation.template.expect("the template's match");
    let matched = json!({
        "x": template.x, "y": template.y, "width": template.width, "height": template.height
    });
    let shown = json!({
        "x": template.x, "y": template.y, "width": template.width, "height": template.height,
        "score": template.score, "raw_score": template.raw_score, "threshold": template.threshold
    });
    assert_eq!(
        serde_json::to_value(&answer).expect("the answer as JSON"),
        json!({
            "target": ICON,
            "passed": true,
            "message": evaluation.message,
            "matched_rect": matched,
            "template": shown,
            "color": null,
            "evaluation": {
                "target": ICON,
                "kind": "Template",
                "passed": true,
                "message": evaluation.message,
                "matched_rect": matched,
                "template": shown,
                "color": null
            },
            "match_metric": "ccoeff_normed"
        })
    );
}
