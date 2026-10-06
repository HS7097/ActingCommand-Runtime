// SPDX-License-Identifier: AGPL-3.0-only

//! In-process ONNX image classifier of the `onnx-classify` family (Workflow #360). Folded in
//! from the former separate NN provider: the same preprocessing (the crop must match the
//! model input exactly, NCHW or NHWC, pixels divided by 255) and positional labels, now with
//! the session built once per model and kept by the Runtime's model cache.

use super::{ENGINE_MODULE, frame_channels};
use actingcommand_onnx_provider_support::InferenceWatchdog;
use actingcommand_vision_ffi::{
    NnClassificationResult, NnEngine, NnInferenceRequest, NnLabel, VisionBackendKind,
    VisionFfiError, VisionFfiErrorCode, VisionFfiResult, VisionFrame,
};
use ort::session::{RunOptions, Session};
use ort::value::{Tensor, TensorElementType, ValueType};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// One loaded classifier.
pub(crate) struct OnnxClassifyModel {
    session: Session,
}

impl OnnxClassifyModel {
    pub(crate) fn new(session: Session) -> Self {
        Self { session }
    }

    fn classify_frame(
        &mut self,
        request: &NnInferenceRequest,
    ) -> Result<NnClassificationResult, ClassifyError> {
        let (shape, layout) = select_input_shape(&self.session, &request.frame)?;
        let input_data = frame_to_tensor(&request.frame, layout)?;
        let input = Tensor::from_array((shape, input_data.into_boxed_slice()))
            .map_err(|err| format!("failed to create ONNX input tensor: {err}"))?;
        let run_options = Arc::new(
            RunOptions::new().map_err(|err| format!("failed to create ONNX run options: {err}"))?,
        );
        let watchdog = InferenceWatchdog::start(
            Arc::clone(&run_options),
            Duration::from_millis(request.timeout_ms),
        );
        let outputs = self
            .session
            .run_with_options(ort::inputs![input], &*run_options);
        if watchdog.timed_out() {
            return Err(ClassifyError::Timeout(
                "ONNXRuntime inference exceeded the configured timeout".to_string(),
            ));
        }
        let outputs = outputs.map_err(|err| format!("ONNXRuntime inference failed: {err}"))?;
        if outputs.len() == 0 {
            return Err("ONNXRuntime inference returned no outputs"
                .to_string()
                .into());
        }
        let (_, scores) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|err| format!("ONNXRuntime first output is not an f32 tensor: {err}"))?;
        Ok(labels_from_scores(&request.labels, scores)?)
    }
}

impl NnEngine for OnnxClassifyModel {
    fn classify(&mut self, request: NnInferenceRequest) -> VisionFfiResult<NnClassificationResult> {
        request.validate()?;
        match catch_unwind(AssertUnwindSafe(|| self.classify_frame(&request))) {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(ClassifyError::Failure(message))) => Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::ProviderFailure,
                ENGINE_MODULE,
                message,
            )),
            Ok(Err(ClassifyError::Timeout(message))) => Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::Timeout,
                ENGINE_MODULE,
                message,
            )),
            Err(_) => Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::ProviderPanic,
                ENGINE_MODULE,
                "NN engine panicked while classifying a frame",
            )),
        }
    }
}

/// Builds the classifier session from the verified bytes: CPU, or CUDA on the resolved device
/// with CPU fallback disabled.
pub(crate) fn load_session(
    model: &[u8],
    path: &Path,
    cuda_ordinal: Option<i32>,
) -> Result<Session, String> {
    let builder = Session::builder()
        .map_err(|err| format!("failed to create ONNXRuntime session builder: {err}"))?
        .with_intra_threads(1)
        .map_err(|err| format!("failed to configure ONNXRuntime intra threads: {err}"))?;
    let mut builder = match cuda_ordinal {
        None => builder,
        Some(ordinal) => builder
            .with_execution_providers([ort::ep::CUDA::default()
                .with_device_id(ordinal)
                .build()
                .error_on_failure()])
            .map_err(|err| {
                format!(
                    "failed to register required CUDA execution provider for device {ordinal}; CPU/DirectML/CoreML fallback is disabled: {err}"
                )
            })?
            .with_disable_cpu_fallback()
            .map_err(|err| format!("failed to disable ONNXRuntime CPU fallback: {err}"))?,
    };
    builder.commit_from_memory(model).map_err(|err| {
        format!(
            "failed to load ONNX classifier model {}: {err}",
            path.display()
        )
    })
}

#[derive(Debug)]
enum ClassifyError {
    Failure(String),
    Timeout(String),
}

impl From<String> for ClassifyError {
    fn from(message: String) -> Self {
        Self::Failure(message)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputLayout {
    Nhwc,
    Nchw,
}

fn select_input_shape(
    session: &Session,
    frame: &VisionFrame,
) -> Result<(Vec<i64>, InputLayout), String> {
    let input = session
        .inputs()
        .first()
        .ok_or_else(|| "ONNX model has no inputs".to_string())?;
    let ValueType::Tensor { ty, shape, .. } = input.dtype() else {
        return Err(format!("ONNX model input {} is not a tensor", input.name()));
    };
    if *ty != TensorElementType::Float32 {
        return Err(format!(
            "ONNX model input {} must be float32, got {ty:?}",
            input.name()
        ));
    }
    if shape.len() != 4 {
        return Err(format!(
            "ONNX model input {} must be rank 4, got shape {shape}",
            input.name()
        ));
    }

    let channels = frame_channels(frame.pixel_format);
    let height = i64::from(frame.height);
    let width = i64::from(frame.width);
    let channels_i64 = i64::try_from(channels).map_err(|err| format!("invalid channels: {err}"))?;

    if dimension_matches(shape[1], channels_i64)
        && dimension_matches(shape[2], height)
        && dimension_matches(shape[3], width)
    {
        return Ok((vec![1, channels_i64, height, width], InputLayout::Nchw));
    }
    if dimension_matches(shape[1], height)
        && dimension_matches(shape[2], width)
        && dimension_matches(shape[3], channels_i64)
    {
        return Ok((vec![1, height, width, channels_i64], InputLayout::Nhwc));
    }

    Err(format!(
        "ONNX model input {} shape {shape} is incompatible with frame {}x{}x{}",
        input.name(),
        frame.width,
        frame.height,
        channels
    ))
}

fn dimension_matches(expected: i64, actual: i64) -> bool {
    expected < 0 || expected == actual
}

fn frame_to_tensor(frame: &VisionFrame, layout: InputLayout) -> Result<Vec<f32>, String> {
    let channels = frame_channels(frame.pixel_format);
    let expected_len = usize::try_from(frame.width)
        .ok()
        .and_then(|width| {
            usize::try_from(frame.height)
                .ok()
                .map(|height| width * height)
        })
        .and_then(|pixels| pixels.checked_mul(channels))
        .ok_or_else(|| "frame dimensions overflow usize".to_string())?;
    if frame.pixels.len() != expected_len {
        return Err(format!(
            "frame pixel buffer length {} does not match expected {expected_len}",
            frame.pixels.len()
        ));
    }
    match layout {
        InputLayout::Nhwc => Ok(frame
            .pixels
            .iter()
            .map(|value| f32::from(*value) / 255.0)
            .collect()),
        InputLayout::Nchw => {
            let pixel_count = expected_len / channels;
            let mut tensor = Vec::with_capacity(expected_len);
            for channel in 0..channels {
                for pixel in 0..pixel_count {
                    tensor.push(f32::from(frame.pixels[pixel * channels + channel]) / 255.0);
                }
            }
            Ok(tensor)
        }
    }
}

fn labels_from_scores(labels: &[String], scores: &[f32]) -> Result<NnClassificationResult, String> {
    if scores.len() != labels.len() {
        return Err(format!(
            "ONNXRuntime output score count {} does not match label count {}",
            scores.len(),
            labels.len()
        ));
    }
    let mut labels: Vec<_> = labels
        .iter()
        .zip(scores.iter())
        .map(|(label, score)| {
            if !score.is_finite() {
                return Err(format!(
                    "ONNXRuntime output score for {label} is not finite"
                ));
            }
            Ok(NnLabel {
                label: label.clone(),
                score: *score,
            })
        })
        .collect::<Result<_, _>>()?;
    labels.sort_by(|left, right| right.score.total_cmp(&left.score));
    Ok(NnClassificationResult {
        labels,
        backend: VisionBackendKind::OnnxRuntime,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_vision_ffi::VisionPixelFormat;

    #[test]
    fn frame_to_tensor_keeps_nhwc_order() {
        let frame = VisionFrame::new(
            1,
            2,
            VisionPixelFormat::Rgb8,
            vec![0, 127, 255, 255, 0, 127],
        )
        .expect("frame");

        let tensor = frame_to_tensor(&frame, InputLayout::Nhwc).expect("tensor");

        assert_eq!(tensor.len(), 6);
        assert_eq!(tensor[0], 0.0);
        assert!((tensor[1] - (127.0 / 255.0)).abs() < f32::EPSILON);
        assert_eq!(tensor[2], 1.0);
        assert_eq!(tensor[3], 1.0);
    }

    #[test]
    fn frame_to_tensor_converts_to_nchw_order() {
        let frame =
            VisionFrame::new(2, 1, VisionPixelFormat::Rgb8, vec![1, 2, 3, 4, 5, 6]).expect("frame");

        let tensor = frame_to_tensor(&frame, InputLayout::Nchw).expect("tensor");

        assert_eq!(
            tensor,
            vec![
                1.0 / 255.0,
                4.0 / 255.0,
                2.0 / 255.0,
                5.0 / 255.0,
                3.0 / 255.0,
                6.0 / 255.0,
            ]
        );
    }

    #[test]
    fn labels_from_scores_sorts_descending() {
        let labels = labels_from_scores(
            &[
                "home".to_string(),
                "unknown".to_string(),
                "battle".to_string(),
            ],
            &[0.2, 0.1, 0.9],
        )
        .expect("labels");

        assert_eq!(labels.labels[0].label, "battle");
        assert_eq!(labels.labels[1].label, "home");
        assert_eq!(labels.backend, VisionBackendKind::OnnxRuntime);
    }

    #[test]
    fn labels_from_scores_rejects_mismatched_count() {
        let err = labels_from_scores(&["home".to_string()], &[0.1, 0.2])
            .expect_err("mismatched labels rejected");

        assert!(err.contains("does not match label count"));
    }
}
