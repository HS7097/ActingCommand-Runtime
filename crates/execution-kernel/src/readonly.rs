// SPDX-License-Identifier: AGPL-3.0-only

//! Pure read-only recognition decisions over caller-supplied resources and scenes.

use actingcommand_contract::{EnvResolved, NeedsDetection};
use actingcommand_page_detector::{PageDetector, PageEvaluation, PageTargetEvaluation};
use actingcommand_recognition::{MatchMetric, Scene};
use actingcommand_recognition_pack::{
    PackRect, RecognitionEvaluator, TargetEvaluation, TargetKind,
};
use serde::Serialize;
use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

pub type ReadonlyRecognitionResult<T> = Result<T, ReadonlyRecognitionError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadonlyRecognitionError {
    message: String,
}

impl ReadonlyRecognitionError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ReadonlyRecognitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "readonly recognition error: {}", self.message)
    }
}

impl Error for ReadonlyRecognitionError {}

/// Side-effect-free recognition engine owned by the production execution domain.
pub struct ReadonlyRecognitionEngine {
    evaluator: RecognitionEvaluator,
    env_resolved: Vec<EnvResolved>,
}

impl ReadonlyRecognitionEngine {
    pub fn new(evaluator: RecognitionEvaluator, env_resolved: Vec<EnvResolved>) -> Self {
        Self {
            evaluator,
            env_resolved,
        }
    }

    pub fn target_requires_scene(&self, target: &str) -> ReadonlyRecognitionResult<bool> {
        self.evaluator
            .target_kind(target)
            .map(|kind| kind != TargetKind::ClickOnly)
            .map_err(pack_error)
    }

    pub fn recognize(
        &self,
        target: &str,
        scene: Option<&Scene>,
    ) -> ReadonlyRecognitionResult<RecognizeResponse> {
        if !self.target_requires_scene(target)? {
            let click = self
                .evaluator
                .get_click_target(target)
                .map_err(pack_error)?;
            return Ok(RecognizeResponse::ClickOnly(RecognizeClickOnlyResponse {
                target: target.to_string(),
                kind: "click_only".to_string(),
                evaluated: false,
                click: rect_response(click),
                match_metric: match_metric_name(self.evaluator.default_match_metric()).to_string(),
                env_resolved: self.env_resolved.clone(),
            }));
        }

        let evaluation = self
            .evaluator
            .evaluate_target(required_scene(scene)?, target)
            .map_err(pack_error)?;
        let evaluation_response = target_evaluation_response(&evaluation);
        Ok(RecognizeResponse::Evaluated(Box::new(
            RecognizeEvaluatedResponse {
                target: target.to_string(),
                passed: evaluation.passed,
                message: evaluation.message.clone(),
                matched_rect: evaluation_response.matched_rect,
                template: evaluation_response.template,
                color: evaluation_response.color,
                evaluation: evaluation_response,
                match_metric: match_metric_name(self.evaluator.default_match_metric()).to_string(),
                needs_detection: (!evaluation.passed)
                    .then(|| {
                        needs_detection(
                            "recognize",
                            "target_below_threshold",
                            target,
                            &self.env_resolved,
                        )
                    })
                    .flatten(),
                env_resolved: self.env_resolved.clone(),
            },
        )))
    }

    pub fn detect_page(
        &self,
        detector: &PageDetector,
        scene: Option<&Scene>,
        check_pages: bool,
    ) -> ReadonlyRecognitionResult<DetectPageOutput> {
        detector.validate(&self.evaluator).map_err(page_error)?;
        if check_pages {
            return Ok(DetectPageOutput {
                response: DetectPageResponse::Check(DetectPageCheckResponse {
                    check_pages: "passed".to_string(),
                }),
                env_resolved: self.env_resolved.clone(),
            });
        }
        let response = detect_current_page(
            &self.evaluator,
            detector,
            required_scene(scene)?,
            "detect-page",
            self.env_resolved.clone(),
        )?;
        Ok(DetectPageOutput {
            response: DetectPageResponse::Detection(Box::new(response)),
            env_resolved: self.env_resolved.clone(),
        })
    }

    pub fn current_page(
        &self,
        detector: &PageDetector,
        scene: &Scene,
    ) -> ReadonlyRecognitionResult<PageDetectionResponse> {
        detector.validate(&self.evaluator).map_err(page_error)?;
        detect_current_page(
            &self.evaluator,
            detector,
            scene,
            "current-page",
            self.env_resolved.clone(),
        )
    }

    pub fn is_visible(
        &self,
        target: &str,
        scene: Option<&Scene>,
    ) -> ReadonlyRecognitionResult<IsVisibleResponse> {
        if !self.target_requires_scene(target)? {
            return Err(ReadonlyRecognitionError::new(format!(
                "target '{target}' is click-only and cannot be evaluated for visibility"
            )));
        }
        let evaluation = self
            .evaluator
            .evaluate_target(required_scene(scene)?, target)
            .map_err(pack_error)?;
        Ok(IsVisibleResponse {
            target: target.to_string(),
            visible: evaluation.passed,
            evaluation: target_evaluation_response(&evaluation),
            match_metric: match_metric_name(self.evaluator.default_match_metric()).to_string(),
            needs_detection: (!evaluation.passed)
                .then(|| {
                    needs_detection(
                        "is-visible",
                        "target_below_threshold",
                        target,
                        &self.env_resolved,
                    )
                })
                .flatten(),
            env_resolved: self.env_resolved.clone(),
        })
    }
}

pub fn detect_current_page(
    evaluator: &RecognitionEvaluator,
    detector: &PageDetector,
    scene: &Scene,
    command: &str,
    env_resolved: Vec<EnvResolved>,
) -> ReadonlyRecognitionResult<PageDetectionResponse> {
    let evaluations = detector
        .evaluate_all(evaluator, scene)
        .map_err(page_error)?;
    let matched = evaluations.iter().find(|evaluation| evaluation.matched);
    let page = matched
        .map(|evaluation| evaluation.page_id.clone())
        .unwrap_or_else(|| "standby".to_string());
    let standby = matched.is_none();
    Ok(PageDetectionResponse {
        page: page.clone(),
        matched: !standby,
        standby,
        evaluations: evaluations.iter().map(page_evaluation_response).collect(),
        recovery_hint: standby.then(|| RecoveryHintResponse {
            action: "wake_safe_point".to_string(),
            point: PointResponse { x: 300, y: 2 },
            note: "CLI does not click automatically".to_string(),
        }),
        req_id: None,
        reco_id: None,
        needs_detection: standby
            .then(|| needs_detection(command, "current_page_unknown", &page, &env_resolved))
            .flatten(),
        env_resolved,
    })
}

pub fn needs_detection(
    command: &str,
    reason: &str,
    subject: &str,
    values: &[EnvResolved],
) -> Option<NeedsDetection> {
    if values.is_empty() {
        return None;
    }
    Some(NeedsDetection {
        status: "needs_detection".to_string(),
        reason: reason.to_string(),
        command: Some(command.to_string()),
        subject: Some(subject.to_string()),
        detector_ids: values
            .iter()
            .map(|value| value.detector_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        keys: values.to_vec(),
        recommended_action: "run_detect".to_string(),
    })
}

pub fn target_evaluation_response(evaluation: &TargetEvaluation) -> TargetEvaluationResponse {
    TargetEvaluationResponse {
        target: evaluation.id.clone(),
        kind: format!("{:?}", evaluation.kind),
        passed: evaluation.passed,
        message: evaluation.message.clone(),
        matched_rect: evaluation.template.map(|template| RectResponse {
            x: template.x,
            y: template.y,
            width: template.width,
            height: template.height,
        }),
        template: evaluation
            .template
            .map(|template| TemplateEvaluationResponse {
                x: template.x,
                y: template.y,
                width: template.width,
                height: template.height,
                score: template.score,
                raw_score: template.raw_score,
                threshold: template.threshold,
            }),
        color: evaluation.color.map(|color| ColorEvaluationResponse {
            distance: color.distance,
            max_distance: color.max_distance,
            mean: color.mean,
            expected: color.expected,
        }),
    }
}

pub fn rect_response(rect: PackRect) -> RectResponse {
    RectResponse {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

fn required_scene(scene: Option<&Scene>) -> ReadonlyRecognitionResult<&Scene> {
    scene.ok_or_else(|| ReadonlyRecognitionError::new("recognition scene is required"))
}

fn page_evaluation_response(evaluation: &PageEvaluation) -> PageEvaluationResponse {
    PageEvaluationResponse {
        page: evaluation.page_id.clone(),
        matched: evaluation.matched,
        message: evaluation.message.clone(),
        any_of_passed: evaluation.any_of_passed,
        any_of_total: evaluation.any_of_total,
        targets: evaluation
            .target_results
            .iter()
            .map(page_target_evaluation_response)
            .collect(),
    }
}

fn page_target_evaluation_response(
    evaluation: &PageTargetEvaluation,
) -> PageTargetEvaluationResponse {
    PageTargetEvaluationResponse {
        id: evaluation.target_id.clone(),
        role: format!("{:?}", evaluation.role),
        passed: evaluation.passed,
        message: evaluation.message.clone(),
    }
}

fn match_metric_name(metric: MatchMetric) -> &'static str {
    match metric {
        MatchMetric::CrossCorrelationNormalized => "ccorr_normed",
        MatchMetric::CorrelationCoefficientNormalized => "ccoeff_normed",
    }
}

fn page_error(error: actingcommand_page_detector::PageDetectorError) -> ReadonlyRecognitionError {
    ReadonlyRecognitionError::new(error.to_string())
}

fn pack_error(
    error: actingcommand_recognition_pack::RecognitionPackError,
) -> ReadonlyRecognitionError {
    ReadonlyRecognitionError::new(error.to_string())
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum RecognizeResponse {
    ClickOnly(RecognizeClickOnlyResponse),
    Evaluated(Box<RecognizeEvaluatedResponse>),
}

#[derive(Debug, Clone, Serialize)]
pub struct RecognizeClickOnlyResponse {
    pub target: String,
    pub kind: String,
    pub evaluated: bool,
    pub click: RectResponse,
    pub match_metric: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub env_resolved: Vec<EnvResolved>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecognizeEvaluatedResponse {
    pub target: String,
    pub passed: bool,
    pub message: String,
    pub matched_rect: Option<RectResponse>,
    pub template: Option<TemplateEvaluationResponse>,
    pub color: Option<ColorEvaluationResponse>,
    pub evaluation: TargetEvaluationResponse,
    pub match_metric: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub env_resolved: Vec<EnvResolved>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs_detection: Option<NeedsDetection>,
}

pub struct DetectPageOutput {
    pub response: DetectPageResponse,
    pub env_resolved: Vec<EnvResolved>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum DetectPageResponse {
    Check(DetectPageCheckResponse),
    Detection(Box<PageDetectionResponse>),
}

#[derive(Debug, Clone, Serialize)]
pub struct DetectPageCheckResponse {
    pub check_pages: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct IsVisibleResponse {
    pub target: String,
    pub visible: bool,
    pub evaluation: TargetEvaluationResponse,
    pub match_metric: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub env_resolved: Vec<EnvResolved>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs_detection: Option<NeedsDetection>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PageDetectionResponse {
    pub page: String,
    pub matched: bool,
    pub standby: bool,
    pub evaluations: Vec<PageEvaluationResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_hint: Option<RecoveryHintResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub req_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reco_id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub env_resolved: Vec<EnvResolved>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs_detection: Option<NeedsDetection>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecoveryHintResponse {
    pub action: String,
    pub point: PointResponse,
    pub note: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct PointResponse {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct RectResponse {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct TargetEvaluationResponse {
    pub target: String,
    pub kind: String,
    pub passed: bool,
    pub message: String,
    pub matched_rect: Option<RectResponse>,
    pub template: Option<TemplateEvaluationResponse>,
    pub color: Option<ColorEvaluationResponse>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct TemplateEvaluationResponse {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub score: f32,
    pub raw_score: f32,
    pub threshold: f32,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ColorEvaluationResponse {
    pub distance: f32,
    pub max_distance: f32,
    pub mean: [u8; 3],
    pub expected: [u8; 3],
}

#[derive(Debug, Clone, Serialize)]
pub struct PageEvaluationResponse {
    pub page: String,
    pub matched: bool,
    pub message: String,
    pub any_of_passed: usize,
    pub any_of_total: usize,
    pub targets: Vec<PageTargetEvaluationResponse>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PageTargetEvaluationResponse {
    pub id: String,
    pub role: String,
    pub passed: bool,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_page_detector::{PageDefinition, PageSet};
    use actingcommand_recognition::ScenePixelFormat;
    use actingcommand_recognition_pack::{
        ClickOnlyTarget, ColorTarget, PackCoordinateSpace, RecognitionDefaults, RecognitionPack,
        RecognitionTarget,
    };

    #[test]
    fn recognize_evaluates_color_target() {
        let engine = engine();
        let response = engine
            .recognize("home_anchor", Some(&scene([255, 0, 0])))
            .expect("recognize");

        let RecognizeResponse::Evaluated(response) = response else {
            panic!("color target must be evaluated");
        };
        assert!(response.passed);
        assert!(response.evaluation.color.is_some());
    }

    #[test]
    fn recognize_click_only_does_not_require_scene() {
        let response = engine()
            .recognize("home_button", None)
            .expect("click-only recognize");

        let RecognizeResponse::ClickOnly(response) = response else {
            panic!("click-only target must not be evaluated");
        };
        assert!(!response.evaluated);
        assert_eq!(response.click.x, 10);
    }

    #[test]
    fn detect_page_and_current_page_share_typed_detection() {
        let engine = engine();
        let detector = detector(&engine.evaluator);
        let scene = scene([255, 0, 0]);
        let detected = engine
            .detect_page(&detector, Some(&scene), false)
            .expect("detect page");
        let DetectPageResponse::Detection(detected) = detected.response else {
            panic!("detect-page must return page detection");
        };
        assert_eq!(detected.page, "fixture/home");
        assert!(detected.matched);

        let current = engine
            .current_page(&detector, &scene)
            .expect("current page");
        assert_eq!(current.page, "fixture/home");
        assert!(current.matched);
    }

    #[test]
    fn is_visible_reports_failed_evaluation_without_fake_success() {
        let response = engine()
            .is_visible("home_anchor", Some(&scene([0, 0, 255])))
            .expect("is visible");

        assert!(!response.visible);
        assert!(!response.evaluation.passed);
    }

    #[test]
    fn evaluated_target_without_scene_fails_visibly() {
        let error = engine()
            .recognize("home_anchor", None)
            .expect_err("scene is required");
        assert!(error.message().contains("scene is required"));
    }

    fn engine() -> ReadonlyRecognitionEngine {
        let pack = RecognitionPack {
            schema_version: "0.3".to_string(),
            game: Some("fixture".to_string()),
            server: Some("test".to_string()),
            locale: None,
            coordinate_space: Some(PackCoordinateSpace {
                width: 1,
                height: 1,
            }),
            defaults: RecognitionDefaults::default(),
            targets: vec![
                RecognitionTarget::Color(ColorTarget {
                    id: "home_anchor".to_string(),
                    region: PackRect {
                        x: 0,
                        y: 0,
                        width: 1,
                        height: 1,
                    },
                    expected: [255, 0, 0],
                    click: None,
                }),
                RecognitionTarget::ClickOnly(ClickOnlyTarget {
                    id: "home_button".to_string(),
                    click: PackRect {
                        x: 10,
                        y: 20,
                        width: 4,
                        height: 6,
                    },
                }),
            ],
        };
        let evaluator = RecognitionEvaluator::new(std::env::temp_dir(), pack).expect("evaluator");
        ReadonlyRecognitionEngine::new(evaluator, Vec::new())
    }

    fn detector(evaluator: &RecognitionEvaluator) -> PageDetector {
        let detector = PageDetector::new(PageSet {
            schema_version: "0.3".to_string(),
            pages: vec![PageDefinition {
                id: "fixture/home".to_string(),
                required: vec!["home_anchor".to_string()],
                any_of: Vec::new(),
                optional: Vec::new(),
                forbidden: Vec::new(),
            }],
        })
        .expect("detector");
        detector.validate(evaluator).expect("detector refs");
        detector
    }

    fn scene(pixel: [u8; 3]) -> Scene {
        Scene::from_pixels(1, 1, &pixel, ScenePixelFormat::Rgb8).expect("scene")
    }
}
