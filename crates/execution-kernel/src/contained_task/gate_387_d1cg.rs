// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #387 D1c-G, the kernel rows of the vision provider failure (Alice 10-14: a code's
// class follows its real cause).
//
// A run's page evaluation whose vision provider fails, or answers invalidly, ends with the class-5
// `contained_task_vision_provider_failed` at every place a run evaluates pages: the home entry
// check of one page and of an entry page set, the run's own page evaluation, and a shared
// interrupt check of the package's pages. An evaluation that fails for another cause stays the
// class-4 `contained_task_recognition_failed`: here a located text that is not among the boxes a
// crowded screen left, so the read cannot conclude (the provider answered as it should). A
// scripted OCR provider answers on a synthetic neutral frame.

use super::*;
use actingcommand_contract::outcome::LadderClass;
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::{
    FsAssetResolver, NnProviderRequest, NnProviderResult, OcrProviderDetection, OcrProviderRequest,
    OcrProviderResult, OcrProviderTextBlock, VisionProviderError, VisionProviderErrorCode,
    load_pack_from_json_str,
};
use serde_json::json;
use std::path::PathBuf;

const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const WIDTH: u32 = 8;
const HEIGHT: u32 = 4;
const HOME: &str = "neutral/home";
const OTHER: &str = "neutral/other";

// What the scripted provider answers to every text read.
#[derive(Debug, Clone, Copy)]
enum Answer {
    // The provider fails with this code.
    Fails(VisionProviderErrorCode),
    // The provider reads a text that is not the expected one, and reports that the screen held
    // more boxes than it kept.
    CrowdedMiss,
}

#[derive(Debug)]
struct ScriptedOcr {
    answer: Answer,
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
        match self.answer {
            Answer::Fails(code) => Err(VisionProviderError::new(code, "scripted provider failure")),
            Answer::CrowdedMiss => Ok(OcrProviderResult {
                ppocr_diagnostics: Vec::new(),
                text: "OTHER".to_owned(),
                blocks: vec![OcrProviderTextBlock {
                    text: "OTHER".to_owned(),
                    rect: request.region,
                    confidence: Some(0.9),
                }],
                confidence: Some(0.9),
                detection: Some(OcrProviderDetection {
                    detected: 65,
                    limit: 64,
                }),
            }),
        }
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

// A navigable task whose page `home` is a located text and whose pages `other` and `terminal` are
// colors; its required entry is `entry`.
fn task(answer: Answer, entry: &[&str]) -> PreparedContainedTask {
    let pack = load_pack_from_json_str(
        &json!({
            "schema_version": "0.7",
            "game": "neutral",
            "server": "test",
            "coordinate_space": {"width": WIDTH, "height": HEIGHT},
            "defaults": {"color_max_distance": 0.0},
            "targets": [
                {
                    "type": "ocr", "id": "page/home",
                    "region": {"x": 0, "y": 0, "width": WIDTH, "height": HEIGHT},
                    "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact",
                    "expected": ["HOME"], "case_sensitive": false, "minimum_confidence": 0.5,
                    "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256,
                    "locate": {"normalize": "identity_v1", "blocks": "detect",
                               "line_merge": {"gap_factor": 0}}
                },
                {"type": "color", "id": "page/other",
                 "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [0, 255, 0]},
                {"type": "color", "id": "page/terminal",
                 "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [0, 0, 255]}
            ]
        })
        .to_string(),
    )
    .expect("recognition pack");
    let evaluator = RecognitionEvaluator::with_vision_provider(
        pack,
        Arc::new(FsAssetResolver::new(PathBuf::new())),
        Arc::new(ScriptedOcr { answer }),
    )
    .expect("recognition evaluator");
    let detector = PageDetector::new(
        serde_json::from_value(json!({
            "schema_version": "0.6",
            "pages": [
                {"id": HOME, "required": ["page/home"]},
                {"id": OTHER, "required": ["page/other"]},
                {"id": "neutral/terminal", "required": ["page/terminal"]}
            ]
        }))
        .expect("page set"),
    )
    .expect("page detector");
    let control: TaskControl = serde_json::from_value(json!({
        "schema_version": CONTROL_SCHEMA,
        "package_id": "neutral.vision.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": HEIGHT},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 5000
    }))
    .expect("task control");
    let program: TaskProgram = serde_json::from_value(json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "target_page": "terminal",
        "operations": [
            {"id": "open_terminal", "from": "home", "to": "terminal",
             "click": {"kind": "point", "x": 1, "y": 0},
             "unguarded_trusted_coordinate": true, "retryable": false},
            {"id": "leave_other", "from": "other", "to": "terminal",
             "click": {"kind": "point", "x": 1, "y": 0},
             "unguarded_trusted_coordinate": true, "retryable": false}
        ]
    }))
    .expect("task program");
    PreparedContainedTask {
        control,
        program,
        evaluator,
        detector,
        entry_page: Some(RequiredEntryPages {
            pages: entry.iter().map(|page| (*page).to_owned()).collect(),
            phases: vec![0; entry.len()],
        }),
        scheduling_outcome: None,
        post_admission_ocr: None,
        post_admission_fields: None,
        package_sha256: "fixture-sha256".into(),
        entry_count: 3,
        task_count: 1,
    }
}

// The frame the device shows: its top-left pixel is no page's color.
fn frame() -> Frame {
    let mut pixels = vec![0_u8; (WIDTH * HEIGHT * 3) as usize];
    pixels[0..3].copy_from_slice(&[255, 0, 0]);
    Frame::from_pixels(
        WIDTH,
        HEIGHT,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
}

// A device that always shows `frame` and takes every input.
struct ShowRuntime;

impl ContainedTaskRuntime for ShowRuntime {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        Ok(frame().into())
    }

    fn input(
        &mut self,
        _action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn record(&mut self, _trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        Ok(())
    }
}

fn task_failure<T: fmt::Debug>(result: Result<T, ContainedTaskRunError<&'static str>>) -> String {
    match result {
        Err(ContainedTaskRunError::Task(error)) => error.code().as_str().to_owned(),
        other => panic!("expected a task failure, got {other:?}"),
    }
}

// The code each place a run evaluates pages ends with when the provider answers `answer`.
fn failures(answer: Answer) -> Vec<(&'static str, String)> {
    let shared_interrupt = task(answer, &[HOME])
        .matching_pages_on_scene(&scene_from_frame(&frame()).expect("scene"), &[HOME])
        .expect_err("the shared interrupt check fails")
        .code()
        .as_str()
        .to_owned();
    vec![
        (
            "the home entry check of one page",
            task_failure(task(answer, &[HOME]).recognize_required_entry(&mut ShowRuntime)),
        ),
        (
            "the home entry check of an entry page set",
            task_failure(task(answer, &[HOME, OTHER]).recognize_required_entry(&mut ShowRuntime)),
        ),
        (
            "the run's page evaluation",
            task_failure(task(answer, &[HOME]).run(&mut ShowRuntime)),
        ),
        ("a shared interrupt check", shared_interrupt),
    ]
}

#[test]
fn d1cg_a_vision_provider_failure_ends_every_page_evaluation_of_a_run_as_class_5() {
    for code in [
        VisionProviderErrorCode::Unavailable,
        VisionProviderErrorCode::Timeout,
        VisionProviderErrorCode::Internal,
        VisionProviderErrorCode::InvalidResponse,
    ] {
        for (place, failure) in failures(Answer::Fails(code)) {
            assert_eq!(
                failure,
                ExecutionKernelCode::ContainedTaskVisionProviderFailed.as_str(),
                "{place}, provider {code:?}"
            );
        }
    }
    assert_eq!(
        ExecutionKernelCode::ContainedTaskVisionProviderFailed.ladder_class(),
        LadderClass::EnvironmentFault
    );
}

#[test]
fn d1cg_a_recognition_that_cannot_conclude_stays_the_class_4_recognition_failure() {
    for (place, failure) in failures(Answer::CrowdedMiss) {
        assert_eq!(
            failure,
            SharedCode::ContainedTaskRecognitionFailed.as_str(),
            "{place}"
        );
    }
    assert_eq!(
        SharedCode::ContainedTaskRecognitionFailed.ladder_class(),
        LadderClass::PackLogicMismatch
    );
}
