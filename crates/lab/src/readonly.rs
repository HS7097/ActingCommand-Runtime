// SPDX-License-Identifier: AGPL-3.0-only

use crate::env_detection::load_scene;
use crate::{Lab, LabPorts};
use actingcommand_contract::{EnvResolved, LabError, LabResult, NeedsDetection};
use actingcommand_page_detector::{
    PageDetector, PageEvaluation, PageTargetEvaluation, load_page_set_from_json_str,
};
use actingcommand_recognition::{MatchMetric, Scene};
use actingcommand_recognition_pack::{
    PackRect, RecognitionEvaluator, TargetEvaluation, TargetKind, load_pack_from_json_str,
};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;

impl<P: LabPorts> Lab<P> {
    pub fn recognize(
        &mut self,
        mut request: crate::RecognizeRequest,
    ) -> LabResult<crate::RecognizeResponse> {
        let (evaluator, env_resolved) = load_evaluator(self, &mut request.input)?;
        if evaluator
            .target_kind(&request.target)
            .map_err(|error| LabError::usage(error.to_string()))?
            == TargetKind::ClickOnly
        {
            let click = evaluator
                .get_click_target(&request.target)
                .map_err(|error| LabError::usage(error.to_string()))?;
            return Ok(crate::RecognizeResponse::ClickOnly(
                crate::RecognizeClickOnlyResponse {
                    target: request.target,
                    kind: "click_only".to_string(),
                    evaluated: false,
                    click: rect_response(click),
                    match_metric: match_metric_name(evaluator.default_match_metric()).to_string(),
                    env_resolved,
                },
            ));
        }
        let scene = recognition_scene(self, &mut request.input)?;
        let evaluation = evaluator
            .evaluate_target(&scene, &request.target)
            .map_err(|error| LabError::usage(error.to_string()))?;
        let evaluation_response = target_evaluation_response(&evaluation);
        Ok(crate::RecognizeResponse::Evaluated(Box::new(
            crate::RecognizeEvaluatedResponse {
                target: request.target.clone(),
                passed: evaluation.passed,
                message: evaluation.message.clone(),
                matched_rect: evaluation_response.matched_rect,
                template: evaluation_response.template,
                color: evaluation_response.color,
                evaluation: evaluation_response,
                match_metric: match_metric_name(evaluator.default_match_metric()).to_string(),
                needs_detection: (!evaluation.passed)
                    .then(|| {
                        needs_detection(
                            "recognize",
                            "target_below_threshold",
                            &request.target,
                            &env_resolved,
                        )
                    })
                    .flatten(),
                env_resolved,
            },
        )))
    }

    pub fn detect_page(
        &mut self,
        mut request: crate::DetectPageRequest,
    ) -> LabResult<crate::DetectPageOutput> {
        let (evaluator, env_resolved) = load_evaluator(self, &mut request.input)?;
        let detector = load_page_detector(
            &request.input,
            "detect-page requires --pages or --resource-root --game",
        )?;
        detector
            .validate(&evaluator)
            .map_err(|error| LabError::usage(error.to_string()))?;
        if request.check_pages {
            return Ok(crate::DetectPageOutput {
                response: crate::DetectPageResponse::Check(crate::DetectPageCheckResponse {
                    check_pages: "passed".to_string(),
                }),
                env_resolved,
            });
        }
        let scene = recognition_scene(self, &mut request.input)?;
        let response = detect_current_page(
            &evaluator,
            &detector,
            &scene,
            "detect-page",
            env_resolved.clone(),
        )?;
        Ok(crate::DetectPageOutput {
            response: crate::DetectPageResponse::Detection(Box::new(response)),
            env_resolved,
        })
    }

    pub fn current_page(
        &mut self,
        mut request: crate::CurrentPageRequest,
    ) -> LabResult<crate::PageDetectionResponse> {
        let (evaluator, env_resolved) = load_evaluator(self, &mut request.input)?;
        let detector = load_page_detector(
            &request.input,
            "semantic page commands require --pages or --resource-root --game",
        )?;
        detector
            .validate(&evaluator)
            .map_err(|error| LabError::usage(error.to_string()))?;
        let scene = recognition_scene(self, &mut request.input)?;
        detect_current_page(&evaluator, &detector, &scene, "current-page", env_resolved)
    }

    pub fn is_visible(
        &mut self,
        mut request: crate::IsVisibleRequest,
    ) -> LabResult<crate::IsVisibleResponse> {
        let (evaluator, env_resolved) = load_evaluator(self, &mut request.input)?;
        if evaluator
            .target_kind(&request.target)
            .map_err(|error| LabError::usage(error.to_string()))?
            == TargetKind::ClickOnly
        {
            return Err(LabError::usage(format!(
                "target '{}' is click-only and cannot be evaluated for visibility",
                request.target
            )));
        }
        let scene = recognition_scene(self, &mut request.input)?;
        let evaluation = evaluator
            .evaluate_target(&scene, &request.target)
            .map_err(|error| LabError::usage(error.to_string()))?;
        Ok(crate::IsVisibleResponse {
            target: request.target.clone(),
            visible: evaluation.passed,
            evaluation: target_evaluation_response(&evaluation),
            match_metric: match_metric_name(evaluator.default_match_metric()).to_string(),
            needs_detection: (!evaluation.passed)
                .then(|| {
                    needs_detection(
                        "is-visible",
                        "target_below_threshold",
                        &request.target,
                        &env_resolved,
                    )
                })
                .flatten(),
            env_resolved,
        })
    }
}

pub(crate) fn load_evaluator<P: LabPorts>(
    lab: &mut Lab<P>,
    input: &mut crate::ReadonlyRecognitionInput,
) -> LabResult<(RecognitionEvaluator, Vec<EnvResolved>)> {
    let pack_json = fs::read_to_string(&input.pack_path).map_err(|error| {
        LabError::usage(format!(
            "failed to read {}: {error}",
            input.pack_path.display()
        ))
    })?;
    let mut pack_value: Value = serde_json::from_str(&pack_json).map_err(|error| {
        LabError::usage(format!(
            "failed to parse {}: {error}",
            input.pack_path.display()
        ))
    })?;
    let env_resolved = lab.resolve_env_markers(input.marker_request.clone(), &mut pack_value)?;
    let resolved_json = serde_json::to_string(&pack_value).map_err(|error| {
        LabError::usage(format!(
            "failed to serialize resolved recognition pack {}: {error}",
            input.pack_path.display()
        ))
    })?;
    let pack = load_pack_from_json_str(&resolved_json)
        .map_err(|error| LabError::usage(error.to_string()))?;
    let evaluator = RecognitionEvaluator::new(input.pack_root.clone(), pack)
        .map_err(|error| LabError::usage(error.to_string()))?;
    Ok((evaluator, env_resolved))
}

pub(crate) fn load_page_detector(
    input: &crate::ReadonlyRecognitionInput,
    missing_message: &str,
) -> LabResult<PageDetector> {
    let path = input
        .pages_path
        .as_ref()
        .ok_or_else(|| LabError::usage(missing_message))?;
    let pages_json = fs::read_to_string(path)
        .map_err(|error| LabError::usage(format!("failed to read {}: {error}", path.display())))?;
    let pages = load_page_set_from_json_str(&pages_json)
        .map_err(|error| LabError::usage(error.to_string()))?;
    PageDetector::new(pages).map_err(|error| LabError::usage(error.to_string()))
}

pub(crate) fn recognition_scene<P: LabPorts>(
    lab: &mut Lab<P>,
    input: &mut crate::ReadonlyRecognitionInput,
) -> LabResult<Scene> {
    if let Some(scene) = input.scene.take() {
        return Ok(scene);
    }
    load_scene(
        lab,
        input.scene_path.as_deref(),
        input.capture_config.as_ref(),
        input.require_fresh,
        input.fresh_delay,
        "command requires --scene <png> or --capture",
    )
}

pub(crate) fn detect_current_page(
    evaluator: &RecognitionEvaluator,
    detector: &PageDetector,
    scene: &Scene,
    command: &str,
    env_resolved: Vec<EnvResolved>,
) -> LabResult<crate::PageDetectionResponse> {
    let evaluations = detector
        .evaluate_all(evaluator, scene)
        .map_err(|error| LabError::usage(error.to_string()))?;
    let matched = evaluations.iter().find(|evaluation| evaluation.matched);
    let page = matched
        .map(|evaluation| evaluation.page_id.clone())
        .unwrap_or_else(|| "standby".to_string());
    let standby = matched.is_none();
    Ok(crate::PageDetectionResponse {
        page: page.clone(),
        matched: !standby,
        standby,
        evaluations: evaluations.iter().map(page_evaluation_response).collect(),
        recovery_hint: standby.then(|| crate::RecoveryHintResponse {
            action: "wake_safe_point".to_string(),
            point: crate::PointResponse { x: 300, y: 2 },
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

pub(crate) fn needs_detection(
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

pub(crate) fn target_evaluation_response(
    evaluation: &TargetEvaluation,
) -> crate::TargetEvaluationResponse {
    crate::TargetEvaluationResponse {
        target: evaluation.id.clone(),
        kind: format!("{:?}", evaluation.kind),
        passed: evaluation.passed,
        message: evaluation.message.clone(),
        matched_rect: evaluation.template.map(|template| crate::RectResponse {
            x: template.x,
            y: template.y,
            width: template.width,
            height: template.height,
        }),
        template: evaluation
            .template
            .map(|template| crate::TemplateEvaluationResponse {
                x: template.x,
                y: template.y,
                width: template.width,
                height: template.height,
                score: template.score,
                raw_score: template.raw_score,
                threshold: template.threshold,
            }),
        color: evaluation
            .color
            .map(|color| crate::ColorEvaluationResponse {
                distance: color.distance,
                max_distance: color.max_distance,
                mean: color.mean,
                expected: color.expected,
            }),
    }
}

fn page_evaluation_response(evaluation: &PageEvaluation) -> crate::PageEvaluationResponse {
    crate::PageEvaluationResponse {
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
) -> crate::PageTargetEvaluationResponse {
    crate::PageTargetEvaluationResponse {
        id: evaluation.target_id.clone(),
        role: format!("{:?}", evaluation.role),
        passed: evaluation.passed,
        message: evaluation.message.clone(),
    }
}

pub(crate) fn rect_response(rect: PackRect) -> crate::RectResponse {
    crate::RectResponse {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

fn match_metric_name(metric: MatchMetric) -> &'static str {
    match metric {
        MatchMetric::CrossCorrelationNormalized => "ccorr_normed",
        MatchMetric::CorrelationCoefficientNormalized => "ccoeff_normed",
    }
}

#[cfg(test)]
mod tests;
