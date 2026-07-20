// SPDX-License-Identifier: AGPL-3.0-only

use crate::env_detection::load_scene;
use crate::{Lab, LabPorts};
use actingcommand_contract::{EnvResolved, LabError, LabResult};
use actingcommand_execution_kernel::{ReadonlyRecognitionEngine, ReadonlyRecognitionError};
use actingcommand_page_detector::PageDetector;
use actingcommand_recognition::Scene;
use actingcommand_recognition_pack::RecognitionEvaluator;

pub(crate) use actingcommand_execution_kernel::{
    needs_detection, rect_response, target_evaluation_response,
};

impl<P: LabPorts> Lab<P> {
    pub fn recognize(
        &mut self,
        mut request: crate::RecognizeRequest,
    ) -> LabResult<crate::RecognizeResponse> {
        let (evaluator, env_resolved) = load_evaluator(self, &mut request.input)?;
        let engine = ReadonlyRecognitionEngine::new(evaluator, env_resolved);
        let scene = if engine
            .target_requires_scene(&request.target)
            .map_err(readonly_error)?
        {
            Some(recognition_scene(self, &mut request.input)?)
        } else {
            None
        };
        engine
            .recognize(&request.target, scene.as_ref())
            .map_err(readonly_error)
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
        let engine = ReadonlyRecognitionEngine::new(evaluator, env_resolved);
        let scene = if request.check_pages {
            None
        } else {
            Some(recognition_scene(self, &mut request.input)?)
        };
        engine
            .detect_page(&detector, scene.as_ref(), request.check_pages)
            .map_err(readonly_error)
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
        let scene = recognition_scene(self, &mut request.input)?;
        ReadonlyRecognitionEngine::new(evaluator, env_resolved)
            .current_page(&detector, &scene)
            .map_err(readonly_error)
    }

    pub fn is_visible(
        &mut self,
        mut request: crate::IsVisibleRequest,
    ) -> LabResult<crate::IsVisibleResponse> {
        let (evaluator, env_resolved) = load_evaluator(self, &mut request.input)?;
        let engine = ReadonlyRecognitionEngine::new(evaluator, env_resolved);
        let scene = if engine
            .target_requires_scene(&request.target)
            .map_err(readonly_error)?
        {
            Some(recognition_scene(self, &mut request.input)?)
        } else {
            None
        };
        engine
            .is_visible(&request.target, scene.as_ref())
            .map_err(readonly_error)
    }
}

pub(crate) fn load_evaluator<P: LabPorts>(
    _lab: &mut Lab<P>,
    input: &mut crate::ReadonlyRecognitionInput,
) -> LabResult<(RecognitionEvaluator, Vec<EnvResolved>)> {
    let evaluator = input.resources.admitted_package().evaluator().clone();
    Ok((evaluator, Vec::new()))
}

pub(crate) fn load_page_detector(
    input: &crate::ReadonlyRecognitionInput,
    missing_message: &str,
) -> LabResult<PageDetector> {
    let _ = missing_message;
    Ok(input.resources.admitted_package().detector().clone())
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
        None,
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
    actingcommand_execution_kernel::detect_current_page(
        evaluator,
        detector,
        scene,
        command,
        env_resolved,
    )
    .map_err(readonly_error)
}

fn readonly_error(error: ReadonlyRecognitionError) -> LabError {
    LabError::usage(error.message())
}

#[cfg(test)]
mod tests;
