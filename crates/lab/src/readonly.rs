// SPDX-License-Identifier: AGPL-3.0-only

use crate::env_detection::load_scene;
use crate::{Lab, LabPorts};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{EnvResolved, LabError, LabErrorClass, LabResult};
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
        let frames = if engine
            .target_requires_scene(&request.target)
            .map_err(readonly_error)?
        {
            recognition_frames(self, &mut request.input)?
        } else {
            Vec::new()
        };
        engine
            .recognize_frames(&request.target, &frame_refs(&frames))
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
        let frames = if request.check_pages {
            Vec::new()
        } else {
            recognition_frames(self, &mut request.input)?
        };
        engine
            .detect_page_frames(&detector, &frame_refs(&frames), request.check_pages)
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
        let frames = recognition_frames(self, &mut request.input)?;
        ReadonlyRecognitionEngine::new(evaluator, env_resolved)
            .current_page_frames(&detector, &frame_refs(&frames))
            .map_err(readonly_error)
    }

    pub fn is_visible(
        &mut self,
        mut request: crate::IsVisibleRequest,
    ) -> LabResult<crate::IsVisibleResponse> {
        let (evaluator, env_resolved) = load_evaluator(self, &mut request.input)?;
        let engine = ReadonlyRecognitionEngine::new(evaluator, env_resolved);
        let frames = if engine
            .target_requires_scene(&request.target)
            .map_err(readonly_error)?
        {
            recognition_frames(self, &mut request.input)?
        } else {
            Vec::new()
        };
        engine
            .is_visible_frames(&request.target, &frame_refs(&frames))
            .map_err(readonly_error)
    }
}

pub(crate) fn load_evaluator<P: LabPorts>(
    _lab: &mut Lab<P>,
    input: &mut crate::ReadonlyRecognitionInput,
) -> LabResult<(RecognitionEvaluator, Vec<EnvResolved>)> {
    let evaluator = input
        .resources
        .loaded_bundle()
        .evaluator()
        .cloned()
        .ok_or_else(|| {
            LabError::package_invalid(
                "externally verified resource bundle has no recognition evaluator",
            )
        })?;
    Ok((evaluator, Vec::new()))
}

pub(crate) fn load_page_detector(
    input: &crate::ReadonlyRecognitionInput,
    missing_message: &str,
) -> LabResult<PageDetector> {
    input
        .resources
        .loaded_bundle()
        .detector()
        .cloned()
        .ok_or_else(|| LabError::package_invalid(missing_message))
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

/// #411 Lab B-2: the input's frame set, oldest first: its earlier frames (`sample_scenes`), then
/// its current frame.
fn recognition_frames<P: LabPorts>(
    lab: &mut Lab<P>,
    input: &mut crate::ReadonlyRecognitionInput,
) -> LabResult<Vec<Scene>> {
    let current = recognition_scene(lab, input)?;
    let mut frames = std::mem::take(&mut input.sample_scenes);
    frames.push(current);
    Ok(frames)
}

fn frame_refs(frames: &[Scene]) -> Vec<&Scene> {
    frames.iter().collect()
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
    if let Some(conflicting_pages) = error.conflicting_pages() {
        return LabError::new(
            LabErrorClass::UsageValidation,
            SharedCode::PageRecognitionConflict.as_str(),
            error.message(),
            &[],
        )
        .with_details(serde_json::json!({ "matched_pages": conflicting_pages }));
    }
    LabError::usage(error.message())
}

#[cfg(test)]
mod tests;
