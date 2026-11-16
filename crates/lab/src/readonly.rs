// SPDX-License-Identifier: AGPL-3.0-only

use crate::env_detection::load_scene;
use crate::{Lab, LabPorts, frames_in_base, package_base};
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
    /// Workflow #392 DA-5cd: the frames are evaluated in the package's coordinate space, a pure
    /// scale of it reduced to it first (`display_base`); the answer names what it was reduced from.
    pub fn recognize(
        &mut self,
        mut request: crate::RecognizeRequest,
    ) -> LabResult<crate::RecognizeResponse> {
        let (evaluator, env_resolved) = load_evaluator(self, &mut request.input)?;
        let base = package_base(&evaluator);
        let engine = ReadonlyRecognitionEngine::new(evaluator, env_resolved);
        let (frames, scaled_from) = if engine
            .target_requires_scene(&request.target)
            .map_err(readonly_error)?
        {
            frames_in_base(base, recognition_frames(self, &mut request.input)?)?
        } else {
            (Vec::new(), None)
        };
        let mut response = engine
            .recognize_frames(&request.target, &frame_refs(&frames))
            .map_err(readonly_error)?;
        if let crate::RecognizeResponse::Evaluated(evaluated) = &mut response {
            evaluated.scaled_from = scaled_from;
        }
        Ok(response)
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
        let base = package_base(&evaluator);
        let engine = ReadonlyRecognitionEngine::new(evaluator, env_resolved);
        let (frames, scaled_from) = if request.check_pages {
            (Vec::new(), None)
        } else {
            frames_in_base(base, recognition_frames(self, &mut request.input)?)?
        };
        let mut output = engine
            .detect_page_frames(&detector, &frame_refs(&frames), request.check_pages)
            .map_err(readonly_error)?;
        if let crate::DetectPageResponse::Detection(detection) = &mut output.response {
            detection.scaled_from = scaled_from;
        }
        Ok(output)
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
        let (frames, scaled_from) = frames_in_base(
            package_base(&evaluator),
            recognition_frames(self, &mut request.input)?,
        )?;
        let mut response = ReadonlyRecognitionEngine::new(evaluator, env_resolved)
            .current_page_frames(&detector, &frame_refs(&frames))
            .map_err(readonly_error)?;
        response.scaled_from = scaled_from;
        Ok(response)
    }

    pub fn is_visible(
        &mut self,
        mut request: crate::IsVisibleRequest,
    ) -> LabResult<crate::IsVisibleResponse> {
        let (evaluator, env_resolved) = load_evaluator(self, &mut request.input)?;
        let base = package_base(&evaluator);
        let engine = ReadonlyRecognitionEngine::new(evaluator, env_resolved);
        let (frames, scaled_from) = if engine
            .target_requires_scene(&request.target)
            .map_err(readonly_error)?
        {
            frames_in_base(base, recognition_frames(self, &mut request.input)?)?
        } else {
            (Vec::new(), None)
        };
        let mut response = engine
            .is_visible_frames(&request.target, &frame_refs(&frames))
            .map_err(readonly_error)?;
        response.scaled_from = scaled_from;
        Ok(response)
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
