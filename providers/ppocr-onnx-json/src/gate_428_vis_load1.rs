// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #428 VIS-LOAD-1 (coordinator work order rd-1110/brief-vis-load-1.md; the coordinator's
// ruling of 11-11: a model's one-time load is not inference): a request's `timeout_ms` bounds its
// inference and not the one-time load of a model that the request triggers. The detector, built on
// the model's first request that detects, is loaded before the request's deadline is taken, so a
// request whose detector load takes longer than its `timeout_ms` succeeds when its inference fits,
// and a request whose inference exceeds its `timeout_ms` still fails with the timeout it fails with
// today. No ONNX Runtime and no wall clock: the request runs the engine's own path
// (`read_request`) over a fake whose load and inferences move the request's clock by the time they
// are told to take. Production cannot reach the fake: it exists only in the test build, and the
// engine's only stages are its ONNX Runtime sessions on the process's monotonic clock.

use super::*;
use actingcommand_vision_ffi::VisionFrame;

/// The request's `timeout_ms`.
const TIMEOUT_MS: u64 = 1_000;
/// The detector's load: five times the request's timeout.
const DETECTOR_LOAD: Duration = Duration::from_millis(5_000);
const TEXT: &str = "ALPHA";

/// A model's stages over a fake: building the detector, a detection and a recognition each move
/// the request's clock by the time they are told to take. A detection finds one box; a recognition
/// reads `TEXT`. Each inference's budget is recorded with its role.
struct FakeStages {
    clock: Duration,
    detector: bool,
    detect_takes: Duration,
    recognize_takes: Duration,
    loads: usize,
    budgets: Vec<(&'static str, Duration)>,
}

impl FakeStages {
    /// A model whose detector is not built yet.
    fn new(detect_takes: Duration, recognize_takes: Duration) -> Self {
        Self {
            clock: Duration::ZERO,
            detector: false,
            detect_takes,
            recognize_takes,
            loads: 0,
            budgets: Vec::new(),
        }
    }
}

impl RequestStages for FakeStages {
    fn clock(&self) -> Duration {
        self.clock
    }

    fn has_detector(&self) -> bool {
        self.detector
    }

    fn ready_detector(&mut self, _diagnostics: &mut PpocrDiagnostics) -> VisionFfiResult<()> {
        self.clock += DETECTOR_LOAD;
        self.loads += 1;
        self.detector = true;
        Ok(())
    }

    fn recognize(
        &mut self,
        _frame: &VisionFrameView<'_>,
        _region: VisionRect,
        budget: Duration,
    ) -> Result<DecodedText, ProviderInvokeError> {
        self.budgets.push(("recognizer", budget));
        self.clock += self.recognize_takes;
        Ok(DecodedText {
            text: TEXT.to_string(),
            confidence: Some(0.95),
        })
    }

    fn detect(
        &mut self,
        _frame: &VisionFrameView<'_>,
        _region: VisionRect,
        _merge_gap_factor: Option<u32>,
        budget: Duration,
    ) -> Result<Vec<DetectedTextBox>, ProviderInvokeError> {
        self.budgets.push(("detector", budget));
        self.clock += self.detect_takes;
        Ok(vec![DetectedTextBox {
            rect: text_box(),
            confidence: 0.9,
        }])
    }
}

/// The one box the fake detector finds.
fn text_box() -> VisionRect {
    VisionRect {
        x: 16,
        y: 8,
        width: 160,
        height: 32,
    }
}

fn frame() -> VisionFrame {
    VisionFrame {
        width: 320,
        height: 80,
        pixel_format: VisionPixelFormat::Rgb8,
        pixels: vec![0; 320 * 80 * 3],
    }
}

/// One request over the whole `frame` that detects first, with `TIMEOUT_MS`, on `stages`, as the
/// engine reads it, with the default folder parameters; and the roles that ran.
fn read_detecting(
    stages: &mut FakeStages,
    frame: &VisionFrame,
) -> (
    Result<OcrInferenceResult, ProviderInvokeError>,
    InvocationRoles,
) {
    let params = PpocrCtcParams::default();
    let languages = vec!["en".to_string()];
    let request = OcrInferenceRequestView {
        frame: frame.view(),
        region: VisionRect {
            x: 0,
            y: 0,
            width: 320,
            height: 80,
        },
        languages: &languages,
        timeout_ms: TIMEOUT_MS,
        blocks: OcrBlockMode::Detect {
            line_merge: OcrLineMerge::Never,
        },
    };
    let mut ran = InvocationRoles::default();
    let result = read_request(
        stages,
        params.detector.merge_gap_factor,
        params.detector.max_boxes,
        request,
        &mut Vec::new(),
        &mut ran,
    );
    (result, ran)
}

#[test]
fn vis_load1_a_detector_load_longer_than_the_timeout_leaves_inference_its_whole_budget() {
    let frame = frame();
    // The inference takes 300 ms of the 1000: the detection 200 ms, the box's recognition 100 ms.
    let mut stages = FakeStages::new(Duration::from_millis(200), Duration::from_millis(100));
    let whole_budget = [
        ("detector", Duration::from_millis(TIMEOUT_MS)),
        ("recognizer", Duration::from_millis(800)),
    ];

    // The model's first request that detects loads the detector, which takes five times its
    // timeout_ms, and only then takes its deadline: the detection gets all of timeout_ms, the
    // recognition what the detection left, and the request succeeds.
    let (result, ran) = read_detecting(&mut stages, &frame);
    let result = result.unwrap_or_else(|error| panic!("the first request that detects: {error:?}"));
    assert_eq!(stages.loads, 1);
    assert_eq!(stages.budgets, whole_budget);
    assert_eq!(
        ran,
        InvocationRoles {
            recognizer: true,
            detector: true,
        }
    );
    assert_eq!(result.text, TEXT);
    assert_eq!(result.blocks.len(), 1);
    assert_eq!(result.blocks[0].rect, text_box());

    // The next request finds the detector built: it loads nothing and has the same budget.
    stages.budgets.clear();
    let (result, _) = read_detecting(&mut stages, &frame);
    let result = result.unwrap_or_else(|error| panic!("the next request: {error:?}"));
    assert_eq!(stages.loads, 1);
    assert_eq!(stages.budgets, whole_budget);
    assert_eq!(result.text, TEXT);
}

#[test]
fn vis_load1_inference_past_the_timeout_still_fails_with_its_timeout() {
    let frame = frame();
    // The detection takes the whole 1000 ms, so the box's recognition cannot start: the request
    // fails with the Timeout of today, whether its detector was loaded first (a load of five
    // times the timeout, which adds nothing to the inference's budget) or was already built.
    for detector_built in [false, true] {
        let mut stages = FakeStages::new(
            Duration::from_millis(TIMEOUT_MS),
            Duration::from_millis(100),
        );
        stages.detector = detector_built;
        let (result, _) = read_detecting(&mut stages, &frame);
        let error = match result {
            Ok(result) => panic!("detector built {detector_built}: succeeded with {result:?}"),
            Err(error) => error.into_ffi_error(),
        };
        assert_eq!(
            error,
            VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::Timeout,
                ENGINE_MODULE,
                "PPOCR recognizer could not start before the configured timeout",
            ),
            "detector built {detector_built}"
        );
        assert_eq!(stages.loads, usize::from(!detector_built));
        assert_eq!(
            stages.budgets,
            [("detector", Duration::from_millis(TIMEOUT_MS))],
            "detector built {detector_built}"
        );
    }
}
