// SPDX-License-Identifier: AGPL-3.0-only

//! ONNXRuntime-backed PPOCR ROI recognizer for the ActingCommand OCR JSON ABI.
//!
//! This provider intentionally stays behind the ActingCommand provider boundary.
//! It does not copy MAA C++ code and does not bundle OCR models or runtime DLLs.

use actingcommand_onnx_provider_support::{
    InferenceWatchdog, OrtRuntimeInitializer, OrtSessionCache,
};
use actingcommand_vision_ffi::{
    FastDeployPpocrInvokeRequest, OcrInferenceResult, OcrTextBlock, VisionBackendKind,
    VisionFfiOwnedBuffer, VisionFrame, VisionPixelFormat, VisionRect,
};
use ort::session::{RunOptions, Session};
use ort::value::{Tensor, TensorElementType, ValueType};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::slice;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

const DEFAULT_REC_HEIGHT: usize = 48;
const DEFAULT_DYNAMIC_REC_WIDTH: usize = 320;
const DETECTOR_DYNAMIC_MAX_SIDE: usize = 960;
const DETECTOR_MIN_SIDE: usize = 32;
const DETECTOR_MULTIPLE: usize = 32;
const DETECTION_THRESHOLD: f32 = 0.30;
const DETECTION_MIN_AREA: usize = 4;
const DETECTION_BOX_PADDING: i32 = 16;
const MAX_DETECTED_TEXT_BOXES: usize = 64;

static ORT_RUNTIME: OrtRuntimeInitializer = OrtRuntimeInitializer::new();
static RECOGNIZER_SESSIONS: OnceLock<OrtSessionCache> = OnceLock::new();
static DETECTOR_SESSIONS: OnceLock<OrtSessionCache> = OnceLock::new();

/// Reads text from a single OCR region through a PPOCR recognizer model.
///
/// # Safety
///
/// `request_ptr` and `request_len` must describe a valid JSON byte slice for
/// the duration of the call. `response_out` must be a valid writable pointer to
/// one `VisionFfiOwnedBuffer`; callers must release any non-empty response with
/// `ac_vision_free_buffer` from this same provider.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ac_fastdeploy_ppocr_read_text_json(
    request_ptr: *const u8,
    request_len: usize,
    response_out: *mut VisionFfiOwnedBuffer,
) -> i32 {
    let result = std::panic::catch_unwind(|| read_text_json(request_ptr, request_len));
    match result {
        Ok(Ok(response)) => write_response(response_out, 0, &response),
        Ok(Err(err)) => write_error(response_out, 1, &err),
        Err(_) => write_error(response_out, 2, "provider panicked while reading OCR text"),
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

fn read_text_json(
    request_ptr: *const u8,
    request_len: usize,
) -> Result<OcrInferenceResult, String> {
    let envelope = read_request(request_ptr, request_len)?;
    envelope.request.validate().map_err(provider_error)?;
    envelope
        .artifacts
        .validate_existing_files()
        .map_err(provider_error)?;
    let runtime_library = select_onnxruntime_library(&envelope.artifacts.runtime_library_paths)?;
    ensure_ort_runtime(runtime_library)?;
    let dictionary = load_dictionary(&envelope.artifacts.dictionary_path)?;
    let recognizer_session = recognizer_sessions()
        .get_or_load(&envelope.artifacts.recognizer_model_path, load_ort_session)?;

    if is_full_frame_region(&envelope.request.frame, envelope.request.region) {
        let detector_session = detector_sessions()
            .get_or_load(&envelope.artifacts.detector_model_path, load_ort_session)?;
        let detected = {
            let mut detector_session = detector_session
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            detect_text_regions(
                &mut detector_session,
                &envelope.request.frame,
                envelope.request.region,
                envelope.request.timeout_ms,
            )?
        };
        let mut blocks = Vec::new();
        let mut recognizer_session = recognizer_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for detected_box in detected.iter().take(MAX_DETECTED_TEXT_BOXES) {
            let decoded = recognize_region(
                &mut recognizer_session,
                &dictionary,
                &envelope.request.frame,
                detected_box.rect,
                envelope.request.timeout_ms,
            )?;
            if !decoded.text.is_empty() {
                blocks.push(OcrTextBlock {
                    text: decoded.text,
                    rect: detected_box.rect,
                    confidence: decoded.confidence.or(Some(detected_box.confidence)),
                });
            }
        }
        let text = blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let confidence = average_confidence(blocks.iter().filter_map(|block| block.confidence));
        Ok(OcrInferenceResult {
            text,
            blocks,
            confidence,
            backend: VisionBackendKind::FastDeployPpocr,
            warnings: Vec::new(),
        })
    } else {
        let mut recognizer_session = recognizer_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let decoded = recognize_region(
            &mut recognizer_session,
            &dictionary,
            &envelope.request.frame,
            envelope.request.region,
            envelope.request.timeout_ms,
        )?;
        let blocks = if decoded.text.is_empty() {
            Vec::new()
        } else {
            vec![OcrTextBlock {
                text: decoded.text.clone(),
                rect: envelope.request.region,
                confidence: decoded.confidence,
            }]
        };

        Ok(OcrInferenceResult {
            text: decoded.text.clone(),
            confidence: decoded.confidence,
            blocks,
            backend: VisionBackendKind::FastDeployPpocr,
            warnings: vec![
                "ppocr_onnx_provider used recognizer-only ROI OCR because a sub-frame region was requested"
                    .to_string(),
            ],
        })
    }
}

fn read_request(
    request_ptr: *const u8,
    request_len: usize,
) -> Result<FastDeployPpocrInvokeRequest, String> {
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
        .map_err(|err| format!("failed to parse FastDeploy/PPOCR JSON envelope: {err}"))
}

fn select_onnxruntime_library(paths: &[PathBuf]) -> Result<&Path, String> {
    if paths.is_empty() {
        return Err("runtime_library_paths must include an ONNXRuntime DLL path".to_string());
    }
    paths
        .iter()
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.to_ascii_lowercase().contains("onnxruntime"))
                .unwrap_or(false)
        })
        .map(PathBuf::as_path)
        .ok_or_else(|| {
            "runtime_library_paths did not include an ONNXRuntime library; configure an explicit onnxruntime DLL path".to_string()
        })
}

fn ensure_ort_runtime(runtime_library: &Path) -> Result<(), String> {
    ORT_RUNTIME.ensure(runtime_library)
}

fn recognizer_sessions() -> &'static OrtSessionCache {
    RECOGNIZER_SESSIONS.get_or_init(OrtSessionCache::new)
}

fn detector_sessions() -> &'static OrtSessionCache {
    DETECTOR_SESSIONS.get_or_init(OrtSessionCache::new)
}

fn load_ort_session(path: &Path) -> Result<Session, String> {
    Session::builder()
        .map_err(|err| format!("failed to create ONNXRuntime session builder: {err}"))?
        .with_intra_threads(1)
        .map_err(|err| format!("failed to configure ONNXRuntime intra threads: {err}"))?
        .commit_from_file(path)
        .map_err(|err| format!("failed to load PPOCR ONNX model {}: {err}", path.display()))
}

fn load_dictionary(path: &Path) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| format!("failed to read PPOCR dictionary {}: {err}", path.display()))?;
    let dictionary: Vec<_> = text.lines().map(ToOwned::to_owned).collect();
    if dictionary.is_empty() {
        return Err(format!("PPOCR dictionary {} is empty", path.display()));
    }
    Ok(dictionary)
}

fn recognize_region(
    session: &mut Session,
    dictionary: &[String],
    frame: &VisionFrame,
    region: VisionRect,
    timeout_ms: u64,
) -> Result<DecodedText, String> {
    let input_shape = select_recognition_input_shape(session, region)?;
    let input_data = frame_region_to_recognition_tensor(frame, region, &input_shape)?;
    let input = Tensor::from_array((input_shape.to_ort_shape(), input_data.into_boxed_slice()))
        .map_err(|err| format!("failed to create PPOCR recognizer input tensor: {err}"))?;
    let run_options = Arc::new(
        RunOptions::new().map_err(|err| format!("failed to create ONNX run options: {err}"))?,
    );
    let _watchdog =
        InferenceWatchdog::start(Arc::clone(&run_options), Duration::from_millis(timeout_ms));
    let outputs = session
        .run_with_options(ort::inputs![input], &*run_options)
        .map_err(|err| format!("PPOCR recognizer inference failed: {err}"))?;
    if outputs.len() == 0 {
        return Err("PPOCR recognizer returned no outputs".to_string());
    }
    let (output_shape, scores) = outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|err| format!("PPOCR recognizer first output is not an f32 tensor: {err}"))?;
    decode_ctc_output(output_shape.as_ref(), scores, dictionary)
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct DetectedTextBox {
    rect: VisionRect,
    confidence: f32,
}

fn detect_text_regions(
    session: &mut Session,
    frame: &VisionFrame,
    region: VisionRect,
    timeout_ms: u64,
) -> Result<Vec<DetectedTextBox>, String> {
    let input_shape = select_detection_input_shape(session, region)?;
    let input_data = frame_region_to_detection_tensor(frame, region, &input_shape)?;
    let input = Tensor::from_array((input_shape.to_ort_shape(), input_data.into_boxed_slice()))
        .map_err(|err| format!("failed to create PPOCR detector input tensor: {err}"))?;
    let run_options = Arc::new(
        RunOptions::new().map_err(|err| format!("failed to create ONNX run options: {err}"))?,
    );
    let _watchdog =
        InferenceWatchdog::start(Arc::clone(&run_options), Duration::from_millis(timeout_ms));
    let outputs = session
        .run_with_options(ort::inputs![input], &*run_options)
        .map_err(|err| format!("PPOCR detector inference failed: {err}"))?;
    if outputs.len() == 0 {
        return Err("PPOCR detector returned no outputs".to_string());
    }
    let (output_shape, scores) = outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|err| format!("PPOCR detector first output is not an f32 tensor: {err}"))?;
    let map = detector_probability_map(output_shape.as_ref(), scores)?;
    let boxes = detect_text_boxes_from_probability_map(&map, region, frame.width, frame.height)?;
    Ok(merge_detected_text_boxes(boxes, frame.width, frame.height))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RecognitionInputShape {
    height: usize,
    width: usize,
}

impl RecognitionInputShape {
    fn to_ort_shape(self) -> Vec<i64> {
        vec![1, 3, self.height as i64, self.width as i64]
    }
}

fn select_recognition_input_shape(
    session: &Session,
    region: VisionRect,
) -> Result<RecognitionInputShape, String> {
    let input = session
        .inputs()
        .first()
        .ok_or_else(|| "PPOCR recognizer model has no inputs".to_string())?;
    let ValueType::Tensor { ty, shape, .. } = input.dtype() else {
        return Err(format!(
            "PPOCR recognizer input {} is not a tensor",
            input.name()
        ));
    };
    if *ty != TensorElementType::Float32 {
        return Err(format!(
            "PPOCR recognizer input {} must be float32, got {ty:?}",
            input.name()
        ));
    }
    if shape.len() != 4 {
        return Err(format!(
            "PPOCR recognizer input {} must be rank 4 NCHW, got shape {shape}",
            input.name()
        ));
    }
    if !dimension_matches(shape[0], 1) || !dimension_matches(shape[1], 3) {
        return Err(format!(
            "PPOCR recognizer input {} must be NCHW with batch 1 and 3 channels, got shape {shape}",
            input.name()
        ));
    }
    let height = positive_or_default(shape[2], DEFAULT_REC_HEIGHT, "recognizer height")?;
    let width = if shape[3] > 0 {
        usize::try_from(shape[3]).map_err(|err| format!("invalid recognizer width: {err}"))?
    } else {
        dynamic_width_for_region(region, height)?
    };
    Ok(RecognitionInputShape { height, width })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DetectionInputShape {
    height: usize,
    width: usize,
}

impl DetectionInputShape {
    fn to_ort_shape(self) -> Vec<i64> {
        vec![1, 3, self.height as i64, self.width as i64]
    }
}

fn select_detection_input_shape(
    session: &Session,
    region: VisionRect,
) -> Result<DetectionInputShape, String> {
    let input = session
        .inputs()
        .first()
        .ok_or_else(|| "PPOCR detector model has no inputs".to_string())?;
    let ValueType::Tensor { ty, shape, .. } = input.dtype() else {
        return Err(format!(
            "PPOCR detector input {} is not a tensor",
            input.name()
        ));
    };
    if *ty != TensorElementType::Float32 {
        return Err(format!(
            "PPOCR detector input {} must be float32, got {ty:?}",
            input.name()
        ));
    }
    if shape.len() != 4 {
        return Err(format!(
            "PPOCR detector input {} must be rank 4 NCHW, got shape {shape}",
            input.name()
        ));
    }
    if !dimension_matches(shape[0], 1) || !dimension_matches(shape[1], 3) {
        return Err(format!(
            "PPOCR detector input {} must be NCHW with batch 1 and 3 channels, got shape {shape}",
            input.name()
        ));
    }
    if shape[2] > 0 && shape[3] > 0 {
        let height =
            usize::try_from(shape[2]).map_err(|err| format!("invalid detector height: {err}"))?;
        let width =
            usize::try_from(shape[3]).map_err(|err| format!("invalid detector width: {err}"))?;
        if height == 0 || width == 0 {
            return Err("detector input dimensions must be non-zero".to_string());
        }
        return Ok(DetectionInputShape { height, width });
    }
    dynamic_detection_shape_for_region(region)
}

fn dimension_matches(expected: i64, actual: i64) -> bool {
    expected < 0 || expected == actual
}

fn positive_or_default(value: i64, default: usize, label: &str) -> Result<usize, String> {
    if value < 0 {
        return Ok(default);
    }
    let value = usize::try_from(value).map_err(|err| format!("invalid {label}: {err}"))?;
    if value == 0 {
        return Err(format!("{label} must be non-zero"));
    }
    Ok(value)
}

fn dynamic_width_for_region(region: VisionRect, height: usize) -> Result<usize, String> {
    if region.height <= 0 {
        return Err("OCR region height must be non-zero".to_string());
    }
    let scaled = ((region.width as f32 / region.height as f32) * height as f32).ceil() as usize;
    Ok(scaled.clamp(32, DEFAULT_DYNAMIC_REC_WIDTH))
}

fn dynamic_detection_shape_for_region(region: VisionRect) -> Result<DetectionInputShape, String> {
    let rect = RectUsize::from_vision_rect(region)?;
    let longest = rect.width.max(rect.height);
    let scale = if longest > DETECTOR_DYNAMIC_MAX_SIDE {
        DETECTOR_DYNAMIC_MAX_SIDE as f32 / longest as f32
    } else {
        1.0
    };
    let width = round_up_to_multiple(
        ((rect.width as f32 * scale).ceil() as usize).max(DETECTOR_MIN_SIDE),
        DETECTOR_MULTIPLE,
    );
    let height = round_up_to_multiple(
        ((rect.height as f32 * scale).ceil() as usize).max(DETECTOR_MIN_SIDE),
        DETECTOR_MULTIPLE,
    );
    Ok(DetectionInputShape { height, width })
}

fn round_up_to_multiple(value: usize, multiple: usize) -> usize {
    value.div_ceil(multiple) * multiple
}

fn frame_region_to_recognition_tensor(
    frame: &VisionFrame,
    region: VisionRect,
    input_shape: &RecognitionInputShape,
) -> Result<Vec<f32>, String> {
    let rect = RectUsize::from_vision_rect(region)?;
    let frame_width =
        usize::try_from(frame.width).map_err(|err| format!("invalid frame width: {err}"))?;
    let frame_height =
        usize::try_from(frame.height).map_err(|err| format!("invalid frame height: {err}"))?;
    if rect.x + rect.width > frame_width || rect.y + rect.height > frame_height {
        return Err("OCR region exceeds frame bounds".to_string());
    }
    let channels = frame_channels(frame.pixel_format);
    let resized_width = ((rect.width as f32 / rect.height as f32) * input_shape.height as f32)
        .ceil()
        .max(1.0) as usize;
    let resized_width = resized_width.min(input_shape.width);
    let plane_size = input_shape.height * input_shape.width;
    let mut tensor = vec![0.0_f32; plane_size * 3];

    for out_y in 0..input_shape.height {
        let src_y = rect.y + (out_y * rect.height / input_shape.height).min(rect.height - 1);
        for out_x in 0..resized_width {
            let src_x = rect.x + (out_x * rect.width / resized_width).min(rect.width - 1);
            let pixel_offset = (src_y * frame_width + src_x) * channels;
            let (r, g, b) = read_rgb_pixel(&frame.pixels, frame.pixel_format, pixel_offset)?;
            let dst = out_y * input_shape.width + out_x;
            tensor[dst] = normalize_rec_pixel(r);
            tensor[plane_size + dst] = normalize_rec_pixel(g);
            tensor[plane_size * 2 + dst] = normalize_rec_pixel(b);
        }
    }
    Ok(tensor)
}

fn frame_region_to_detection_tensor(
    frame: &VisionFrame,
    region: VisionRect,
    input_shape: &DetectionInputShape,
) -> Result<Vec<f32>, String> {
    let rect = RectUsize::from_vision_rect(region)?;
    let frame_width =
        usize::try_from(frame.width).map_err(|err| format!("invalid frame width: {err}"))?;
    let frame_height =
        usize::try_from(frame.height).map_err(|err| format!("invalid frame height: {err}"))?;
    if rect.x + rect.width > frame_width || rect.y + rect.height > frame_height {
        return Err("OCR detector region exceeds frame bounds".to_string());
    }
    let channels = frame_channels(frame.pixel_format);
    let plane_size = input_shape.height * input_shape.width;
    let mut tensor = vec![0.0_f32; plane_size * 3];

    for out_y in 0..input_shape.height {
        let src_y = rect.y + (out_y * rect.height / input_shape.height).min(rect.height - 1);
        for out_x in 0..input_shape.width {
            let src_x = rect.x + (out_x * rect.width / input_shape.width).min(rect.width - 1);
            let pixel_offset = (src_y * frame_width + src_x) * channels;
            let (r, g, b) = read_rgb_pixel(&frame.pixels, frame.pixel_format, pixel_offset)?;
            let dst = out_y * input_shape.width + out_x;
            tensor[dst] = normalize_det_pixel(r, 0);
            tensor[plane_size + dst] = normalize_det_pixel(g, 1);
            tensor[plane_size * 2 + dst] = normalize_det_pixel(b, 2);
        }
    }
    Ok(tensor)
}

#[derive(Debug, Clone, Copy)]
struct RectUsize {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

impl RectUsize {
    fn from_vision_rect(rect: VisionRect) -> Result<Self, String> {
        if rect.x < 0 || rect.y < 0 || rect.width <= 0 || rect.height <= 0 {
            return Err("OCR region must have non-negative origin and positive size".to_string());
        }
        Ok(Self {
            x: rect.x as usize,
            y: rect.y as usize,
            width: rect.width as usize,
            height: rect.height as usize,
        })
    }
}

fn frame_channels(pixel_format: VisionPixelFormat) -> usize {
    match pixel_format {
        VisionPixelFormat::Rgb8 => 3,
        VisionPixelFormat::Rgba8 => 4,
        VisionPixelFormat::Gray8 => 1,
    }
}

fn read_rgb_pixel(
    pixels: &[u8],
    pixel_format: VisionPixelFormat,
    offset: usize,
) -> Result<(u8, u8, u8), String> {
    match pixel_format {
        VisionPixelFormat::Rgb8 => read_channels(pixels, offset)
            .map(|channels: [u8; 3]| (channels[0], channels[1], channels[2])),
        VisionPixelFormat::Rgba8 => read_channels(pixels, offset)
            .map(|channels: [u8; 4]| (channels[0], channels[1], channels[2])),
        VisionPixelFormat::Gray8 => pixels
            .get(offset)
            .copied()
            .map(|value| (value, value, value))
            .ok_or_else(|| "frame pixel buffer ended while reading gray pixel".to_string()),
    }
}

fn read_channels<const N: usize>(pixels: &[u8], offset: usize) -> Result<[u8; N], String> {
    let end = offset
        .checked_add(N)
        .ok_or_else(|| "frame pixel offset overflow".to_string())?;
    let slice = pixels
        .get(offset..end)
        .ok_or_else(|| "frame pixel buffer ended while reading color pixel".to_string())?;
    slice
        .try_into()
        .map_err(|_| "failed to read color pixel".to_string())
}

fn normalize_rec_pixel(value: u8) -> f32 {
    f32::from(value) / 127.5 - 1.0
}

fn normalize_det_pixel(value: u8, channel: usize) -> f32 {
    const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
    const STD: [f32; 3] = [0.229, 0.224, 0.225];
    (f32::from(value) / 255.0 - MEAN[channel]) / STD[channel]
}

#[derive(Debug, Clone, PartialEq)]
struct ProbabilityMap {
    width: usize,
    height: usize,
    values: Vec<f32>,
}

fn detector_probability_map(shape: &[i64], scores: &[f32]) -> Result<ProbabilityMap, String> {
    let dims: Vec<_> = shape
        .iter()
        .map(|dim| {
            usize::try_from(*dim).map_err(|err| format!("invalid detector output dimension: {err}"))
        })
        .collect::<Result<_, _>>()?;
    let (height, width) = match dims.as_slice() {
        [1, 1, height, width] if height * width == scores.len() => (*height, *width),
        [1, height, width] if height * width == scores.len() => (*height, *width),
        [height, width] if height * width == scores.len() => (*height, *width),
        _ => {
            return Err(format!(
                "unsupported PPOCR detector output shape {shape:?} for {} scores",
                scores.len()
            ));
        }
    };
    if height == 0 || width == 0 {
        return Err("PPOCR detector output dimensions must be non-zero".to_string());
    }
    let mut values = Vec::with_capacity(scores.len());
    for score in scores {
        if !score.is_finite() {
            return Err("PPOCR detector output contains a non-finite score".to_string());
        }
        values.push(to_probability(*score));
    }
    Ok(ProbabilityMap {
        width,
        height,
        values,
    })
}

fn to_probability(value: f32) -> f32 {
    if (0.0..=1.0).contains(&value) {
        value
    } else {
        1.0 / (1.0 + (-value).exp())
    }
}

fn detect_text_boxes_from_probability_map(
    map: &ProbabilityMap,
    region: VisionRect,
    frame_width: u32,
    frame_height: u32,
) -> Result<Vec<DetectedTextBox>, String> {
    let mut visited = vec![false; map.values.len()];
    let mut boxes = Vec::new();
    for index in 0..map.values.len() {
        if visited[index] || map.values[index] < DETECTION_THRESHOLD {
            continue;
        }
        let component = collect_component(map, index, &mut visited);
        if component.area < DETECTION_MIN_AREA {
            continue;
        }
        let rect = component_to_vision_rect(&component, map, region, frame_width, frame_height)?;
        boxes.push(DetectedTextBox {
            rect,
            confidence: component.max_score,
        });
    }
    boxes.sort_by_key(|text_box| (text_box.rect.y, text_box.rect.x));
    Ok(boxes)
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct MapComponent {
    min_x: usize,
    min_y: usize,
    max_x: usize,
    max_y: usize,
    area: usize,
    max_score: f32,
}

fn collect_component(
    map: &ProbabilityMap,
    start_index: usize,
    visited: &mut [bool],
) -> MapComponent {
    let mut queue = VecDeque::from([start_index]);
    visited[start_index] = true;
    let mut component = MapComponent {
        min_x: start_index % map.width,
        min_y: start_index / map.width,
        max_x: start_index % map.width,
        max_y: start_index / map.width,
        area: 0,
        max_score: map.values[start_index],
    };

    while let Some(index) = queue.pop_front() {
        let x = index % map.width;
        let y = index / map.width;
        component.min_x = component.min_x.min(x);
        component.min_y = component.min_y.min(y);
        component.max_x = component.max_x.max(x);
        component.max_y = component.max_y.max(y);
        component.area += 1;
        component.max_score = component.max_score.max(map.values[index]);

        for (next_x, next_y) in neighbors4(x, y, map.width, map.height) {
            let next_index = next_y * map.width + next_x;
            if visited[next_index] || map.values[next_index] < DETECTION_THRESHOLD {
                continue;
            }
            visited[next_index] = true;
            queue.push_back(next_index);
        }
    }
    component
}

fn neighbors4(x: usize, y: usize, width: usize, height: usize) -> Vec<(usize, usize)> {
    let mut neighbors = Vec::with_capacity(4);
    if x > 0 {
        neighbors.push((x - 1, y));
    }
    if x + 1 < width {
        neighbors.push((x + 1, y));
    }
    if y > 0 {
        neighbors.push((x, y - 1));
    }
    if y + 1 < height {
        neighbors.push((x, y + 1));
    }
    neighbors
}

fn component_to_vision_rect(
    component: &MapComponent,
    map: &ProbabilityMap,
    region: VisionRect,
    frame_width: u32,
    frame_height: u32,
) -> Result<VisionRect, String> {
    let scale_x = region.width as f32 / map.width as f32;
    let scale_y = region.height as f32 / map.height as f32;
    let left = region.x + (component.min_x as f32 * scale_x).floor() as i32;
    let top = region.y + (component.min_y as f32 * scale_y).floor() as i32;
    let right = region.x + ((component.max_x + 1) as f32 * scale_x).ceil() as i32;
    let bottom = region.y + ((component.max_y + 1) as f32 * scale_y).ceil() as i32;
    padded_rect(left, top, right, bottom, frame_width, frame_height)
}

fn padded_rect(
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    frame_width: u32,
    frame_height: u32,
) -> Result<VisionRect, String> {
    let frame_width =
        i32::try_from(frame_width).map_err(|err| format!("invalid frame width: {err}"))?;
    let frame_height =
        i32::try_from(frame_height).map_err(|err| format!("invalid frame height: {err}"))?;
    let x = (left - DETECTION_BOX_PADDING).max(0);
    let y = (top - DETECTION_BOX_PADDING).max(0);
    let right = (right + DETECTION_BOX_PADDING).min(frame_width);
    let bottom = (bottom + DETECTION_BOX_PADDING).min(frame_height);
    if right <= x || bottom <= y {
        return Err("detected OCR box collapsed after clamping".to_string());
    }
    Ok(VisionRect {
        x,
        y,
        width: right - x,
        height: bottom - y,
    })
}

fn merge_detected_text_boxes(
    mut boxes: Vec<DetectedTextBox>,
    frame_width: u32,
    frame_height: u32,
) -> Vec<DetectedTextBox> {
    boxes.sort_by_key(|text_box| (rect_center_y(text_box.rect), text_box.rect.x));
    let mut merged: Vec<DetectedTextBox> = Vec::new();
    for text_box in boxes {
        if let Some(last) = merged.last_mut()
            && should_merge_text_boxes(last.rect, text_box.rect)
            && let Ok(rect) = union_rect(last.rect, text_box.rect, frame_width, frame_height)
        {
            last.rect = rect;
            last.confidence = last.confidence.max(text_box.confidence);
            continue;
        }
        merged.push(text_box);
    }
    merged.sort_by_key(|text_box| (text_box.rect.y, text_box.rect.x));
    merged
}

fn should_merge_text_boxes(left: VisionRect, right: VisionRect) -> bool {
    let vertical_overlap = (rect_bottom(left).min(rect_bottom(right)) - left.y.max(right.y)).max(0);
    let min_height = left.height.min(right.height).max(1);
    let horizontal_gap = if right.x >= rect_right(left) {
        right.x - rect_right(left)
    } else if left.x >= rect_right(right) {
        left.x - rect_right(right)
    } else {
        0
    };
    vertical_overlap * 100 >= min_height * 35
        && horizontal_gap <= left.height.max(right.height).max(24) * 3
}

fn union_rect(
    left: VisionRect,
    right: VisionRect,
    frame_width: u32,
    frame_height: u32,
) -> Result<VisionRect, String> {
    padded_rect(
        left.x.min(right.x),
        left.y.min(right.y),
        rect_right(left).max(rect_right(right)),
        rect_bottom(left).max(rect_bottom(right)),
        frame_width,
        frame_height,
    )
}

fn rect_right(rect: VisionRect) -> i32 {
    rect.x + rect.width
}

fn rect_bottom(rect: VisionRect) -> i32 {
    rect.y + rect.height
}

fn rect_center_y(rect: VisionRect) -> i32 {
    rect.y + rect.height / 2
}

fn is_full_frame_region(frame: &VisionFrame, region: VisionRect) -> bool {
    region.x == 0
        && region.y == 0
        && region.width == frame.width as i32
        && region.height == frame.height as i32
}

fn average_confidence(values: impl Iterator<Item = f32>) -> Option<f32> {
    let mut count = 0;
    let mut sum = 0.0;
    for value in values {
        count += 1;
        sum += value;
    }
    if count == 0 {
        None
    } else {
        Some(sum / count as f32)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct DecodedText {
    text: String,
    confidence: Option<f32>,
}

fn decode_ctc_output(
    shape: &[i64],
    scores: &[f32],
    dictionary: &[String],
) -> Result<DecodedText, String> {
    let (steps, class_count) = output_layout(shape, scores.len())?;
    if class_count < 2 {
        return Err(
            "PPOCR recognizer output class count must include blank plus labels".to_string(),
        );
    }
    if class_count > dictionary.len() + 2 {
        return Err(format!(
            "PPOCR recognizer output has {class_count} classes but dictionary has {} labels",
            dictionary.len()
        ));
    }
    let mut text = String::new();
    let mut previous_index = 0_usize;
    let mut confidences = Vec::new();
    for step in 0..steps {
        let row = &scores[step * class_count..(step + 1) * class_count];
        let (index, confidence) = argmax_with_softmax_confidence(row)?;
        if index == 0 || index > dictionary.len() {
            previous_index = 0;
            continue;
        }
        if index != previous_index {
            let label = dictionary
                .get(index - 1)
                .ok_or_else(|| format!("PPOCR recognizer class {index} has no dictionary entry"))?;
            text.push_str(label);
            confidences.push(confidence);
        }
        previous_index = index;
    }
    let confidence = if confidences.is_empty() {
        None
    } else {
        Some(confidences.iter().sum::<f32>() / confidences.len() as f32)
    };
    Ok(DecodedText { text, confidence })
}

fn output_layout(shape: &[i64], score_len: usize) -> Result<(usize, usize), String> {
    let dims: Vec<_> = shape
        .iter()
        .map(|dim| usize::try_from(*dim).map_err(|err| format!("invalid output dimension: {err}")))
        .collect::<Result<_, _>>()?;
    match dims.as_slice() {
        [1, steps, class_count] if steps * class_count == score_len => Ok((*steps, *class_count)),
        [steps, class_count] if steps * class_count == score_len => Ok((*steps, *class_count)),
        _ => Err(format!(
            "unsupported PPOCR recognizer output shape {shape:?} for {score_len} scores"
        )),
    }
}

fn argmax_with_softmax_confidence(row: &[f32]) -> Result<(usize, f32), String> {
    if row.is_empty() {
        return Err("empty PPOCR recognizer output row".to_string());
    }
    if row.iter().any(|score| !score.is_finite()) {
        return Err("PPOCR recognizer output contains a non-finite score".to_string());
    }
    let mut best_index = 0;
    let mut best_value = row[0];
    for (index, value) in row.iter().copied().enumerate().skip(1) {
        if value > best_value {
            best_index = index;
            best_value = value;
        }
    }
    let row_sum: f32 = row.iter().sum();
    if row_sum.is_finite()
        && (0.99..=1.01).contains(&row_sum)
        && row.iter().all(|value| (0.0..=1.0).contains(value))
    {
        return Ok((best_index, best_value));
    }

    let max_value = row
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, |left, right| left.max(right));
    let exp_sum: f32 = row.iter().map(|value| (*value - max_value).exp()).sum();
    if !exp_sum.is_finite() || exp_sum <= 0.0 {
        return Err("PPOCR recognizer output softmax sum is invalid".to_string());
    }
    Ok((best_index, (best_value - max_value).exp() / exp_sum))
}

fn provider_error(err: impl std::fmt::Display) -> String {
    format!("{err}")
}

fn write_response<T: serde::Serialize>(
    response_out: *mut VisionFfiOwnedBuffer,
    status: i32,
    value: &T,
) -> i32 {
    match serde_json::to_vec(value) {
        Ok(bytes) => write_bytes(response_out, status, bytes),
        Err(err) => write_error(
            response_out,
            2,
            &format!("failed to serialize provider response JSON: {err}"),
        ),
    }
}

fn write_error(response_out: *mut VisionFfiOwnedBuffer, status: i32, message: &str) -> i32 {
    write_bytes(response_out, status, message.as_bytes().to_vec())
}

fn write_bytes(response_out: *mut VisionFfiOwnedBuffer, status: i32, bytes: Vec<u8>) -> i32 {
    if response_out.is_null() {
        return 2;
    }
    let mut bytes = bytes;
    let buffer = VisionFfiOwnedBuffer {
        data: bytes.as_mut_ptr(),
        len: bytes.len(),
        capacity: bytes.capacity(),
    };
    std::mem::forget(bytes);
    // SAFETY: response_out is checked for null and points to caller-owned
    // writable storage according to the ActingCommand JSON ABI.
    unsafe {
        *response_out = buffer;
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_onnxruntime_path_by_name() {
        let paths = vec![
            PathBuf::from("fastdeploy_ppocr_maa.dll"),
            PathBuf::from("onnxruntime_maa.dll"),
        ];

        assert_eq!(
            select_onnxruntime_library(&paths).expect("path"),
            Path::new("onnxruntime_maa.dll")
        );
    }

    #[test]
    fn rejects_runtime_library_list_without_onnxruntime_name() {
        let paths = vec![
            PathBuf::from("fastdeploy_ppocr_maa.dll"),
            PathBuf::from("helper.dll"),
        ];

        let err =
            select_onnxruntime_library(&paths).expect_err("missing onnxruntime path rejected");

        assert!(err.contains("did not include an ONNXRuntime library"));
    }

    #[test]
    fn decodes_ctc_output_with_blank_and_repeated_labels() {
        let dictionary = vec!["A".to_string(), "B".to_string()];
        let scores = vec![
            4.0, 0.0, 0.0, //
            0.0, 5.0, 0.0, //
            0.0, 6.0, 0.0, //
            5.0, 0.0, 0.0, //
            0.0, 0.0, 5.0, //
        ];

        let decoded = decode_ctc_output(&[1, 5, 3], &scores, &dictionary).expect("decode");

        assert_eq!(decoded.text, "AB");
        assert!(decoded.confidence.expect("confidence") > 0.9);
    }

    #[test]
    fn rejects_output_without_dictionary_entry() {
        let dictionary = vec!["A".to_string()];
        let scores = vec![0.0; 8];

        let err =
            decode_ctc_output(&[1, 2, 4], &scores, &dictionary).expect_err("too many classes");

        assert!(err.contains("dictionary"));
    }

    #[test]
    fn treats_extra_ppocr_class_as_special_token() {
        let dictionary = vec!["A".to_string()];
        let scores = vec![
            0.0, 5.0, 0.0, //
            0.0, 0.0, 5.0, //
            0.0, 5.0, 0.0, //
        ];

        let decoded = decode_ctc_output(&[1, 3, 3], &scores, &dictionary).expect("decode");

        assert_eq!(decoded.text, "AA");
    }

    #[test]
    fn confidence_uses_existing_probability_distribution() {
        let (index, confidence) =
            argmax_with_softmax_confidence(&[0.1, 0.8, 0.1]).expect("confidence");

        assert_eq!(index, 1);
        assert!((confidence - 0.8).abs() < 0.001);
    }

    #[test]
    fn converts_region_to_nchw_recognition_tensor() {
        let frame = VisionFrame {
            width: 2,
            height: 1,
            pixel_format: VisionPixelFormat::Rgb8,
            pixels: vec![0, 127, 255, 255, 127, 0],
        };
        let shape = RecognitionInputShape {
            height: 1,
            width: 2,
        };

        let tensor = frame_region_to_recognition_tensor(
            &frame,
            VisionRect {
                x: 0,
                y: 0,
                width: 2,
                height: 1,
            },
            &shape,
        )
        .expect("tensor");

        assert_eq!(tensor.len(), 6);
        assert!((tensor[0] + 1.0).abs() < 0.001);
        assert!((tensor[1] - 1.0).abs() < 0.001);
        assert!((tensor[4] - 1.0).abs() < 0.001);
        assert!((tensor[5] + 1.0).abs() < 0.001);
    }

    #[test]
    fn parses_detector_probability_map() {
        let map = detector_probability_map(&[1, 1, 2, 3], &[0.0, 0.25, 0.5, 0.75, 1.0, 2.0])
            .expect("map");

        assert_eq!(map.width, 3);
        assert_eq!(map.height, 2);
        assert_eq!(map.values[4], 1.0);
        assert!(map.values[5] > 0.88);
    }

    #[test]
    fn detects_and_merges_nearby_text_components() {
        let mut values = vec![0.0; 8 * 3];
        for y in 1..3 {
            for x in 1..3 {
                values[y * 8 + x] = 0.9;
            }
            for x in 4..6 {
                values[y * 8 + x] = 0.8;
            }
        }
        let map = ProbabilityMap {
            width: 8,
            height: 3,
            values,
        };

        let boxes = detect_text_boxes_from_probability_map(
            &map,
            VisionRect {
                x: 0,
                y: 0,
                width: 80,
                height: 30,
            },
            80,
            30,
        )
        .expect("boxes");
        let merged = merge_detected_text_boxes(boxes, 80, 30);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].rect.x, 0);
        assert!(merged[0].rect.width >= 60);
        assert!(merged[0].confidence > 0.89);
    }

    #[test]
    fn distinguishes_full_frame_from_sub_frame_region() {
        let frame = VisionFrame {
            width: 320,
            height: 80,
            pixel_format: VisionPixelFormat::Rgb8,
            pixels: vec![0; 320 * 80 * 3],
        };

        assert!(is_full_frame_region(
            &frame,
            VisionRect {
                x: 0,
                y: 0,
                width: 320,
                height: 80,
            }
        ));
        assert!(!is_full_frame_region(
            &frame,
            VisionRect {
                x: 1,
                y: 0,
                width: 319,
                height: 80,
            }
        ));
    }
}
