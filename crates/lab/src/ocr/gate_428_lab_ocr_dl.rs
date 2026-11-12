// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #428 LAB-OCR-DL (register §1.331; coordinator work order
// rd-1112/brief-lab-ocr-deadline.md; the coordinator's rulings of 11-12 on board #428, not the
// owner's words): an offline evaluation of a saved scene may, on its switch, raise each OCR
// target's inference deadline to the contract's most, 60 000 ms; raised, not removed.
// - `lab_ocr_dl_an_inference_longer_than_the_timeout_fails_without_the_switch_and_passes_with_it`:
//   the evaluation the Lab's readers run on a saved scene (`ReadonlyRecognitionEngine`, as
//   `Lab::recognize` runs it) over an evaluator given the provider as actinglab's offline package
//   load gives it, for an OCR target that declares `timeout_ms` 1 000 and whose inference takes
//   3 000 ms. Given the provider itself, the request's deadline is the declared 1 000 and the target
//   fails with the engine's Timeout of today; given it through `with_ocr_deadline_max`, the
//   request's deadline is 60 000 and the target passes.
// The provider is a fake whose inference takes the time it is told and fails as the engine does
// when that is longer than the request's deadline; no ONNX Runtime, no model file, no clock, no
// sleep.

use super::*;
use actingcommand_execution_kernel::{ReadonlyRecognitionEngine, RecognizeResponse};
use actingcommand_recognition::Scene;
use actingcommand_recognition_pack::{
    FsAssetResolver, NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, RecognitionEvaluator, VisionProviderError, VisionProviderErrorCode,
    load_pack_from_json_str,
};
use serde_json::json;
use std::sync::Mutex;

const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
/// The OCR target the scene is evaluated for.
const TARGET: &str = "label/title";
/// The target's declared `timeout_ms`.
const TIMEOUT_MS: u64 = 1_000;
/// How long the fake's inference takes: three times the declared `timeout_ms`.
const INFERENCE_MS: u64 = 3_000;
/// What the engine fails a request with when its inference used the whole deadline and left its
/// recognition none, as the kernel's provider hands it on (the text the resource repositories' CI
/// met).
const ENGINE_TIMEOUT: &str = "fatal vision FFI error in ppocr_ctc_engine: PPOCR recognizer could not start before the configured timeout";

/// An engine whose inference takes [`INFERENCE_MS`]: a request whose deadline is shorter fails with
/// the engine's `Timeout`, one whose deadline holds it reads "Start" at confidence 0.95. It records
/// each request's deadline.
#[derive(Debug, Default)]
struct SlowEngine {
    deadlines: Mutex<Vec<u64>>,
}

impl SlowEngine {
    /// The deadline of every OCR request it was given, in order.
    fn deadlines(&self) -> Vec<u64> {
        self.deadlines.lock().expect("the deadlines").clone()
    }
}

impl VisionProvider for SlowEngine {
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
        self.deadlines
            .lock()
            .expect("the deadlines")
            .push(request.timeout_ms);
        if INFERENCE_MS > request.timeout_ms {
            return Err(VisionProviderError::new(
                VisionProviderErrorCode::Timeout,
                ENGINE_TIMEOUT,
            ));
        }
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: "Start".to_owned(),
            blocks: vec![OcrProviderTextBlock {
                text: "Start".to_owned(),
                rect: request.region,
                confidence: Some(0.95),
            }],
            confidence: Some(0.95),
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

/// A neutral pack with one OCR target that declares [`TIMEOUT_MS`], evaluated with `provider`.
fn evaluator(provider: Arc<dyn VisionProvider>) -> RecognitionEvaluator {
    let pack = load_pack_from_json_str(
        &json!({
            "schema_version": "0.7",
            "coordinate_space": {"width": 64, "height": 32},
            "targets": [{
                "type": "ocr", "id": TARGET, "region": {"x": 8, "y": 8, "width": 32, "height": 16},
                "languages": ["en"], "timeout_ms": TIMEOUT_MS, "match_mode": "exact",
                "expected": ["Start"], "case_sensitive": false, "minimum_confidence": 0.8,
                "model_ref": "neutral-ocr", "model_sha256": MODEL_SHA256
            }]
        })
        .to_string(),
    )
    .expect("the neutral pack");
    RecognitionEvaluator::with_vision_provider(
        pack,
        Arc::new(FsAssetResolver::new(PathBuf::new())),
        provider,
    )
    .expect("the evaluator")
}

/// The Lab readers' recognition of [`TARGET`] on the saved scene `scene`, with `provider`; a
/// failure as its message.
fn recognized(
    provider: Arc<dyn VisionProvider>,
    scene: &Scene,
) -> Result<RecognizeResponse, String> {
    ReadonlyRecognitionEngine::new(evaluator(provider), Vec::new())
        .recognize_frames(TARGET, &[scene])
        .map_err(|error| error.message().to_owned())
}

#[test]
fn lab_ocr_dl_an_inference_longer_than_the_timeout_fails_without_the_switch_and_passes_with_it() {
    let scene = Scene::from_rgb8(64, 32, &[255u8, 0, 0].repeat(64 * 32)).expect("the saved scene");

    // Without the switch: the request's deadline is the declared timeout_ms, which the inference
    // outlasts, and the target fails with the engine's Timeout, as it fails today.
    let declared = Arc::new(SlowEngine::default());
    let failure = match recognized(Arc::clone(&declared), &scene) {
        Ok(answer) => panic!("without the switch the target was recognized: {answer:?}"),
        Err(message) => message,
    };
    assert!(
        failure.contains(&format!(
            "ocr provider failed for target '{TARGET}' with Timeout: {ENGINE_TIMEOUT}"
        )),
        "{failure}"
    );
    assert_eq!(declared.deadlines(), [TIMEOUT_MS]);

    // With it: the same request reaches the engine with the deadline 60 000, which the inference
    // fits, and the target passes; raised to the contract's most, not removed.
    let raised = Arc::new(SlowEngine::default());
    match recognized(
        with_ocr_deadline_max(Arc::clone(&raised) as Arc<dyn VisionProvider>),
        &scene,
    ) {
        Ok(RecognizeResponse::Evaluated(answer)) => {
            assert!(answer.passed, "{answer:?}");
            assert_eq!(answer.target, TARGET);
        }
        other => panic!("with the switch the target was not evaluated as passing: {other:?}"),
    }
    assert_eq!(raised.deadlines(), [60_000]);
}
