// SPDX-License-Identifier: AGPL-3.0-only

//! ONNXRuntime-backed implementation of the ActingCommand NN JSON ABI.
//!
//! This provider is intentionally separate from `actingcommand-vision-ffi`.
//! The Runtime loads it through the documented local dynamic-library boundary,
//! and the provider loads a reviewed `onnxruntime.dll` from the artifact
//! manifest instead of bundling or implicitly discovering one.

use actingcommand_onnx_provider_support::{InferenceWatchdog, OrtRuntimeInitializer};
use actingcommand_vision_ffi::{
    NnClassificationResult, NnLabel, OnnxRuntimeInvokeRequest, VisionBackendKind, VisionFfiError,
    VisionFfiOwnedBuffer, VisionFrame, VisionPixelFormat,
};
use ort::session::{RunOptions, Session};
use ort::value::{Tensor, TensorElementType, ValueType};
use serde::Serialize;
use std::path::Path;
use std::slice;
use std::sync::Arc;
use std::time::Duration;

const MODULE: &str = "onnxruntime-json-provider";
static ORT_RUNTIME: OrtRuntimeInitializer = OrtRuntimeInitializer::new();

/// Classifies a frame through an ONNXRuntime model using the ActingCommand JSON ABI.
///
/// # Safety
///
/// `request_ptr` and `request_len` must describe a valid JSON byte slice for
/// the duration of the call. `response_out` must be a valid writable pointer to
/// one `VisionFfiOwnedBuffer`; callers must release any non-empty response with
/// `ac_vision_free_buffer` from this same provider.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ac_onnxruntime_classify_json(
    request_ptr: *const u8,
    request_len: usize,
    response_out: *mut VisionFfiOwnedBuffer,
) -> i32 {
    invoke_provider(response_out, || classify_json(request_ptr, request_len))
}

fn invoke_provider<F>(response_out: *mut VisionFfiOwnedBuffer, invoke: F) -> i32
where
    F: FnOnce() -> Result<NnClassificationResult, String> + std::panic::UnwindSafe,
{
    let result = std::panic::catch_unwind(invoke);
    match result {
        Ok(Ok(response)) => write_response(response_out, 0, &response),
        Ok(Err(err)) => write_error(response_out, 1, &err),
        Err(_) => write_error(response_out, 2, "provider panicked while classifying frame"),
    }
}

/// Releases a buffer allocated by this provider.
///
/// # Safety
///
/// The buffer must have been returned by this provider and must not have been
/// released before. Passing buffers from another provider or arbitrary pointers
/// is undefined behavior.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ac_vision_free_buffer(buffer: VisionFfiOwnedBuffer) {
    if buffer.data.is_null() || buffer.capacity == 0 {
        return;
    }
    // SAFETY: every buffer returned by this provider is allocated from a Vec
    // with the exact pointer, length, and capacity stored in the ABI struct.
    unsafe {
        drop(Vec::from_raw_parts(
            buffer.data,
            buffer.len,
            buffer.capacity,
        ));
    }
}

fn classify_json(
    request_ptr: *const u8,
    request_len: usize,
) -> Result<NnClassificationResult, String> {
    let envelope = read_request(request_ptr, request_len)?;
    envelope.request.validate().map_err(provider_error)?;
    envelope
        .artifacts
        .validate_existing_files()
        .map_err(provider_error)?;
    let runtime_library = envelope
        .artifacts
        .runtime_library_path
        .as_deref()
        .ok_or_else(|| {
            "runtime_library_path is required for the ONNXRuntime provider".to_string()
        })?;
    ensure_ort_runtime(runtime_library)?;

    let mut session = Session::builder()
        .map_err(|err| format!("failed to create ONNXRuntime session builder: {err}"))?
        .with_intra_threads(1)
        .map_err(|err| format!("failed to configure ONNXRuntime intra threads: {err}"))?
        .commit_from_file(&envelope.artifacts.model_path)
        .map_err(|err| {
            format!(
                "failed to load ONNX model {}: {err}",
                envelope.artifacts.model_path.display()
            )
        })?;

    let (shape, layout) = select_input_shape(&session, &envelope.request.frame)?;
    let input_data = frame_to_tensor(&envelope.request.frame, layout)?;
    let input = Tensor::from_array((shape, input_data.into_boxed_slice()))
        .map_err(|err| format!("failed to create ONNX input tensor: {err}"))?;
    let run_options = Arc::new(
        RunOptions::new().map_err(|err| format!("failed to create ONNX run options: {err}"))?,
    );
    let _watchdog = InferenceWatchdog::start(
        Arc::clone(&run_options),
        Duration::from_millis(envelope.request.timeout_ms),
    );
    let outputs = session
        .run_with_options(ort::inputs![input], &*run_options)
        .map_err(|err| format!("ONNXRuntime inference failed: {err}"))?;
    if outputs.len() == 0 {
        return Err("ONNXRuntime inference returned no outputs".to_string());
    }
    let output = &outputs[0];
    let (_, scores) = output
        .try_extract_tensor::<f32>()
        .map_err(|err| format!("ONNXRuntime first output is not an f32 tensor: {err}"))?;
    labels_from_scores(&envelope.request.labels, scores)
}

fn ensure_ort_runtime(runtime_library: &Path) -> Result<(), String> {
    ORT_RUNTIME.ensure(runtime_library)
}

fn read_request(
    request_ptr: *const u8,
    request_len: usize,
) -> Result<OnnxRuntimeInvokeRequest, String> {
    if request_len == 0 {
        return Err("empty JSON request".to_string());
    }
    if request_ptr.is_null() {
        return Err("null JSON request pointer".to_string());
    }
    // SAFETY: the caller provides a request pointer and length that must remain
    // valid for this call according to the ActingCommand JSON ABI.
    let bytes = unsafe { slice::from_raw_parts(request_ptr, request_len) };
    serde_json::from_slice(bytes)
        .map_err(|err| format!("failed to parse ONNXRuntime JSON envelope: {err}"))
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

fn frame_channels(pixel_format: VisionPixelFormat) -> usize {
    match pixel_format {
        VisionPixelFormat::Rgb8 => 3,
        VisionPixelFormat::Rgba8 => 4,
        VisionPixelFormat::Gray8 => 1,
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

fn write_response<T: Serialize>(
    response_out: *mut VisionFfiOwnedBuffer,
    status: i32,
    response: &T,
) -> i32 {
    match serde_json::to_vec(response) {
        Ok(bytes) => write_buffer(response_out, bytes, status),
        Err(err) => write_error(
            response_out,
            1,
            &format!("failed to serialize provider response: {err}"),
        ),
    }
}

fn write_error(response_out: *mut VisionFfiOwnedBuffer, status: i32, message: &str) -> i32 {
    let err = VisionFfiError::fatal(MODULE, message);
    write_response(response_out, status, &err)
}

fn write_buffer(response_out: *mut VisionFfiOwnedBuffer, mut bytes: Vec<u8>, status: i32) -> i32 {
    if response_out.is_null() {
        return 1;
    }
    let buffer = VisionFfiOwnedBuffer {
        data: bytes.as_mut_ptr(),
        len: bytes.len(),
        capacity: bytes.capacity(),
    };
    std::mem::forget(bytes);
    // SAFETY: response_out is checked non-null and points to caller-owned
    // storage for one ABI buffer struct.
    unsafe {
        response_out.write(buffer);
    }
    status
}

fn provider_error(err: VisionFfiError) -> String {
    format!("{err}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_vision_ffi::VisionFrame;

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

    #[test]
    fn exported_classify_rejects_invalid_json() {
        let mut response = VisionFfiOwnedBuffer::default();
        let status = unsafe {
            ac_onnxruntime_classify_json(
                b"{".as_ptr(),
                1,
                &mut response as *mut VisionFfiOwnedBuffer,
            )
        };

        assert_eq!(status, 1);
        assert!(response.len > 0);
        let bytes = unsafe { slice::from_raw_parts(response.data, response.len) };
        let text = std::str::from_utf8(bytes).expect("utf8");
        assert!(text.contains("failed to parse ONNXRuntime JSON envelope"));
        unsafe {
            ac_vision_free_buffer(response);
        }
    }

    #[test]
    fn exported_classify_reports_provider_panic() {
        let mut response = VisionFfiOwnedBuffer::default();

        let status = invoke_provider(&mut response, || panic!("injected provider panic"));

        assert_eq!(status, 2);
        let bytes = unsafe { slice::from_raw_parts(response.data, response.len) };
        let text = std::str::from_utf8(bytes).expect("utf8");
        assert!(text.contains("panicked"));
        unsafe {
            ac_vision_free_buffer(response);
        }
    }
}
