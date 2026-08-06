// SPDX-License-Identifier: AGPL-3.0-only

//! Safe Rust boundary for future OCR and NN engines.
//!
//! This crate deliberately stops at the process/FFI contract surface. The real
//! FastDeploy/PPOCR and ONNXRuntime bindings must live behind this boundary so
//! runtime callers cannot silently substitute mock recognition for production
//! OCR or NN results.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod artifacts;
pub mod ffi;

pub use artifacts::*;
pub use ffi::*;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;

pub type VisionFfiResult<T> = Result<T, VisionFfiError>;

const MAX_REQUEST_TIMEOUT_MS: u64 = 60_000;
const MAX_REQUEST_LANGUAGES: usize = 8;
const MAX_REQUEST_LABELS: usize = 256;
const MAX_REQUEST_STRING_BYTES: usize = 4_096;
const MAX_OCR_TEXT_BYTES: usize = 64 * 1024;
const MAX_OCR_BLOCKS: usize = 1_024;
const MAX_NN_RESULTS: usize = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisionFfiErrorSeverity {
    Fatal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisionFfiErrorCode {
    InvalidRequest,
    ProviderUnavailable,
    ProviderFailure,
    ProviderPanic,
    Timeout,
    ModelMismatch,
    InvalidResponse,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisionFfiError {
    severity: VisionFfiErrorSeverity,
    code: VisionFfiErrorCode,
    module: &'static str,
    message: String,
}

impl VisionFfiError {
    pub fn fatal(module: &'static str, message: impl Into<String>) -> Self {
        Self::fatal_with_code(VisionFfiErrorCode::Internal, module, message)
    }

    pub fn fatal_with_code(
        code: VisionFfiErrorCode,
        module: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity: VisionFfiErrorSeverity::Fatal,
            code,
            module,
            message: message.into(),
        }
    }

    pub fn severity(&self) -> VisionFfiErrorSeverity {
        self.severity
    }

    pub fn module(&self) -> &'static str {
        self.module
    }

    pub fn code(&self) -> VisionFfiErrorCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for VisionFfiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.severity {
            VisionFfiErrorSeverity::Fatal => {
                write!(
                    f,
                    "fatal vision FFI error in {}: {}",
                    self.module, self.message
                )
            }
        }
    }
}

impl Error for VisionFfiError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisionBackendKind {
    TestDouble,
    FastDeployPpocr,
    OnnxRuntime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisionPixelFormat {
    Rgb8,
    Rgba8,
    Gray8,
}

impl VisionPixelFormat {
    fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgb8 => 3,
            Self::Rgba8 => 4,
            Self::Gray8 => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisionFrame {
    pub width: u32,
    pub height: u32,
    pub pixel_format: VisionPixelFormat,
    #[serde(with = "base64_pixels")]
    pub pixels: Vec<u8>,
}

impl VisionFrame {
    pub fn new(
        width: u32,
        height: u32,
        pixel_format: VisionPixelFormat,
        pixels: Vec<u8>,
    ) -> VisionFfiResult<Self> {
        validate_frame_pixels(width, height, pixel_format, pixels.len())?;
        Ok(Self {
            width,
            height,
            pixel_format,
            pixels,
        })
    }

    pub fn validate(&self) -> VisionFfiResult<()> {
        validate_frame_pixels(
            self.width,
            self.height,
            self.pixel_format,
            self.pixels.len(),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisionRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl VisionRect {
    pub fn full_frame(frame: &VisionFrame) -> VisionFfiResult<Self> {
        let width = i32::try_from(frame.width)
            .map_err(|_| VisionFfiError::fatal("vision-frame", "frame width exceeds i32 range"))?;
        let height = i32::try_from(frame.height)
            .map_err(|_| VisionFfiError::fatal("vision-frame", "frame height exceeds i32 range"))?;
        Ok(Self {
            x: 0,
            y: 0,
            width,
            height,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OcrInferenceRequest {
    pub frame: VisionFrame,
    pub region: VisionRect,
    pub languages: Vec<String>,
    pub timeout_ms: u64,
}

impl OcrInferenceRequest {
    pub fn validate(&self) -> VisionFfiResult<()> {
        self.frame.validate()?;
        validate_rect(self.region, self.frame.width, self.frame.height)?;
        if self.languages.is_empty() {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::InvalidRequest,
                "ocr",
                "OCR request must include at least one language",
            ));
        }
        if self.languages.len() > MAX_REQUEST_LANGUAGES
            || self.languages.iter().any(|language| {
                language.trim().is_empty() || language.len() > MAX_REQUEST_STRING_BYTES
            })
        {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::InvalidRequest,
                "ocr",
                format!(
                    "OCR request languages must contain 1..={MAX_REQUEST_LANGUAGES} non-blank values of at most {MAX_REQUEST_STRING_BYTES} bytes"
                ),
            ));
        }
        if self.timeout_ms == 0 || self.timeout_ms > MAX_REQUEST_TIMEOUT_MS {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::InvalidRequest,
                "ocr",
                format!("OCR request timeout_ms must be in 1..={MAX_REQUEST_TIMEOUT_MS}"),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrTextBlock {
    pub text: String,
    pub rect: VisionRect,
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrInferenceResult {
    pub text: String,
    pub blocks: Vec<OcrTextBlock>,
    pub confidence: Option<f32>,
    pub backend: VisionBackendKind,
    pub warnings: Vec<String>,
}

impl OcrInferenceResult {
    pub fn validate(&self, request: &OcrInferenceRequest) -> VisionFfiResult<()> {
        if self.text.len() > MAX_OCR_TEXT_BYTES {
            return Err(invalid_response(format!(
                "OCR text exceeds {MAX_OCR_TEXT_BYTES} bytes"
            )));
        }
        validate_optional_unit_score(self.confidence, "OCR confidence")?;
        if self.blocks.len() > MAX_OCR_BLOCKS {
            return Err(invalid_response(format!(
                "OCR result contains {} blocks, limit is {MAX_OCR_BLOCKS}",
                self.blocks.len()
            )));
        }
        for (index, block) in self.blocks.iter().enumerate() {
            if block.text.len() > MAX_REQUEST_STRING_BYTES {
                return Err(invalid_response(format!(
                    "OCR block[{index}] text exceeds {MAX_REQUEST_STRING_BYTES} bytes"
                )));
            }
            validate_optional_unit_score(
                block.confidence,
                &format!("OCR block[{index}] confidence"),
            )?;
            validate_rect(block.rect, request.frame.width, request.frame.height)?;
            if !rect_contains(request.region, block.rect) {
                return Err(invalid_response(format!(
                    "OCR block[{index}] is outside the requested region"
                )));
            }
        }
        Ok(())
    }
}

pub trait OcrEngine {
    fn read_text(&mut self, request: OcrInferenceRequest) -> VisionFfiResult<OcrInferenceResult>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NnInferenceRequest {
    pub frame: VisionFrame,
    pub model_id: String,
    pub labels: Vec<String>,
    pub timeout_ms: u64,
}

impl NnInferenceRequest {
    pub fn validate(&self) -> VisionFfiResult<()> {
        self.frame.validate()?;
        if self.model_id.trim().is_empty() {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::InvalidRequest,
                "nn",
                "NN request model_id must be non-empty",
            ));
        }
        if self.model_id.len() > MAX_REQUEST_STRING_BYTES {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::InvalidRequest,
                "nn",
                format!("NN request model_id exceeds {MAX_REQUEST_STRING_BYTES} bytes"),
            ));
        }
        if self.labels.is_empty() {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::InvalidRequest,
                "nn",
                "NN request must include at least one candidate label",
            ));
        }
        if self.labels.len() > MAX_REQUEST_LABELS
            || self
                .labels
                .iter()
                .any(|label| label.trim().is_empty() || label.len() > MAX_REQUEST_STRING_BYTES)
        {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::InvalidRequest,
                "nn",
                format!(
                    "NN request labels must contain 1..={MAX_REQUEST_LABELS} non-blank values of at most {MAX_REQUEST_STRING_BYTES} bytes"
                ),
            ));
        }
        if self.timeout_ms == 0 || self.timeout_ms > MAX_REQUEST_TIMEOUT_MS {
            return Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::InvalidRequest,
                "nn",
                format!("NN request timeout_ms must be in 1..={MAX_REQUEST_TIMEOUT_MS}"),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NnLabel {
    pub label: String,
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NnClassificationResult {
    pub labels: Vec<NnLabel>,
    pub backend: VisionBackendKind,
}

impl NnClassificationResult {
    pub fn validate(&self) -> VisionFfiResult<()> {
        if self.labels.len() > MAX_NN_RESULTS {
            return Err(invalid_response(format!(
                "NN result contains {} labels, limit is {MAX_NN_RESULTS}",
                self.labels.len()
            )));
        }
        for (index, label) in self.labels.iter().enumerate() {
            if label.label.trim().is_empty() || label.label.len() > MAX_REQUEST_STRING_BYTES {
                return Err(invalid_response(format!(
                    "NN result label[{index}] must be non-blank and at most {MAX_REQUEST_STRING_BYTES} bytes"
                )));
            }
            validate_unit_score(label.score, &format!("NN result label[{index}] score"))?;
        }
        Ok(())
    }
}

pub trait NnEngine {
    fn classify(&mut self, request: NnInferenceRequest) -> VisionFfiResult<NnClassificationResult>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisionFfiRouteDecision {
    pub route: &'static str,
    pub ocr_backend: VisionBackendKind,
    pub nn_backend: VisionBackendKind,
    pub gpu_enabled: bool,
    pub directml_enabled: bool,
    pub bundled_artifacts: bool,
    pub expected_size_delta_mb: (u16, u16),
}

pub fn r1_r3_route_decision() -> VisionFfiRouteDecision {
    VisionFfiRouteDecision {
        route: "ffi_boundary_then_fastdeploy_ppocr_and_onnxruntime",
        ocr_backend: VisionBackendKind::FastDeployPpocr,
        nn_backend: VisionBackendKind::OnnxRuntime,
        gpu_enabled: true,
        directml_enabled: false,
        bundled_artifacts: false,
        expected_size_delta_mb: (150, 250),
    }
}

#[derive(Debug, Default)]
pub struct UnavailableOcrBackend;

impl OcrEngine for UnavailableOcrBackend {
    fn read_text(&mut self, request: OcrInferenceRequest) -> VisionFfiResult<OcrInferenceResult> {
        request.validate()?;
        Err(VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::ProviderUnavailable,
            "ocr",
            "FastDeploy/PPOCR backend is not linked or configured",
        ))
    }
}

#[derive(Debug, Default)]
pub struct UnavailableNnBackend;

impl NnEngine for UnavailableNnBackend {
    fn classify(&mut self, request: NnInferenceRequest) -> VisionFfiResult<NnClassificationResult> {
        request.validate()?;
        Err(VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::ProviderUnavailable,
            "nn",
            "ONNXRuntime backend is not linked or configured",
        ))
    }
}

pub struct VisionFfiBoundary<O, N> {
    ocr: O,
    nn: N,
}

impl<O, N> VisionFfiBoundary<O, N> {
    pub fn new(ocr: O, nn: N) -> Self {
        Self { ocr, nn }
    }
}

impl<O, N> VisionFfiBoundary<O, N>
where
    O: OcrEngine,
    N: NnEngine,
{
    pub fn read_text(
        &mut self,
        request: OcrInferenceRequest,
    ) -> VisionFfiResult<OcrInferenceResult> {
        self.ocr.read_text(request)
    }

    pub fn classify(
        &mut self,
        request: NnInferenceRequest,
    ) -> VisionFfiResult<NnClassificationResult> {
        self.nn.classify(request)
    }
}

fn validate_frame_pixels(
    width: u32,
    height: u32,
    pixel_format: VisionPixelFormat,
    pixel_len: usize,
) -> VisionFfiResult<()> {
    if width == 0 || height == 0 {
        return Err(VisionFfiError::fatal(
            "vision-frame",
            format!("frame dimensions must be non-zero: {width}x{height}"),
        ));
    }
    let expected = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(pixel_format.bytes_per_pixel()))
        .ok_or_else(|| {
            VisionFfiError::fatal(
                "vision-frame",
                format!("frame dimensions overflow: {width}x{height}"),
            )
        })?;
    if pixel_len != expected {
        return Err(VisionFfiError::fatal(
            "vision-frame",
            format!(
                "frame pixel length mismatch for {width}x{height}: got {pixel_len}, expected {expected}"
            ),
        ));
    }
    Ok(())
}

mod base64_pixels {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn serialize<S>(pixels: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&encode(pixels))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        decode(&encoded).map_err(D::Error::custom)
    }

    fn encode(bytes: &[u8]) -> String {
        let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let b0 = chunk[0];
            let b1 = chunk.get(1).copied().unwrap_or(0);
            let b2 = chunk.get(2).copied().unwrap_or(0);
            output.push(TABLE[(b0 >> 2) as usize] as char);
            output.push(TABLE[(((b0 & 0b0000_0011) << 4) | (b1 >> 4)) as usize] as char);
            if chunk.len() > 1 {
                output.push(TABLE[(((b1 & 0b0000_1111) << 2) | (b2 >> 6)) as usize] as char);
            } else {
                output.push('=');
            }
            if chunk.len() > 2 {
                output.push(TABLE[(b2 & 0b0011_1111) as usize] as char);
            } else {
                output.push('=');
            }
        }
        output
    }

    fn decode(encoded: &str) -> Result<Vec<u8>, String> {
        if !encoded.len().is_multiple_of(4) {
            return Err("base64 pixel payload length must be a multiple of 4".to_string());
        }
        let bytes = encoded.as_bytes();
        let mut output = Vec::with_capacity(encoded.len() / 4 * 3);
        for quartet in bytes.chunks(4) {
            let v0 = decode_value(quartet[0])?;
            let v1 = decode_value(quartet[1])?;
            let pad2 = quartet[2] == b'=';
            let pad3 = quartet[3] == b'=';
            let v2 = if pad2 { 0 } else { decode_value(quartet[2])? };
            let v3 = if pad3 { 0 } else { decode_value(quartet[3])? };
            if pad2 && !pad3 {
                return Err("base64 pixel payload has invalid padding".to_string());
            }
            output.push((v0 << 2) | (v1 >> 4));
            if !pad2 {
                output.push(((v1 & 0b0000_1111) << 4) | (v2 >> 2));
            }
            if !pad3 {
                output.push(((v2 & 0b0000_0011) << 6) | v3);
            }
        }
        Ok(output)
    }

    fn decode_value(byte: u8) -> Result<u8, String> {
        match byte {
            b'A'..=b'Z' => Ok(byte - b'A'),
            b'a'..=b'z' => Ok(byte - b'a' + 26),
            b'0'..=b'9' => Ok(byte - b'0' + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err(format!(
                "base64 pixel payload contains invalid byte 0x{byte:02x}"
            )),
        }
    }

    #[cfg(test)]
    pub(super) fn encoded_len(bytes: &[u8]) -> usize {
        encode(bytes).len()
    }
}

fn validate_rect(rect: VisionRect, frame_width: u32, frame_height: u32) -> VisionFfiResult<()> {
    if rect.x < 0 || rect.y < 0 {
        return Err(VisionFfiError::fatal(
            "vision-rect",
            format!(
                "rect coordinates must be non-negative: ({}, {})",
                rect.x, rect.y
            ),
        ));
    }
    if rect.width <= 0 || rect.height <= 0 {
        return Err(VisionFfiError::fatal(
            "vision-rect",
            format!(
                "rect dimensions must be positive: {}x{}",
                rect.width, rect.height
            ),
        ));
    }

    let x = u32::try_from(rect.x)
        .map_err(|_| VisionFfiError::fatal("vision-rect", "rect x cannot be converted to u32"))?;
    let y = u32::try_from(rect.y)
        .map_err(|_| VisionFfiError::fatal("vision-rect", "rect y cannot be converted to u32"))?;
    let width = u32::try_from(rect.width).map_err(|_| {
        VisionFfiError::fatal("vision-rect", "rect width cannot be converted to u32")
    })?;
    let height = u32::try_from(rect.height).map_err(|_| {
        VisionFfiError::fatal("vision-rect", "rect height cannot be converted to u32")
    })?;
    let right = x
        .checked_add(width)
        .ok_or_else(|| VisionFfiError::fatal("vision-rect", "rect x + width overflows u32"))?;
    let bottom = y
        .checked_add(height)
        .ok_or_else(|| VisionFfiError::fatal("vision-rect", "rect y + height overflows u32"))?;

    if right > frame_width || bottom > frame_height {
        return Err(VisionFfiError::fatal(
            "vision-rect",
            format!(
                "rect {}x{} at ({}, {}) exceeds frame {}x{}",
                width, height, x, y, frame_width, frame_height
            ),
        ));
    }
    Ok(())
}

fn validate_optional_unit_score(score: Option<f32>, label: &str) -> VisionFfiResult<()> {
    if let Some(score) = score {
        validate_unit_score(score, label)?;
    }
    Ok(())
}

fn validate_unit_score(score: f32, label: &str) -> VisionFfiResult<()> {
    if !score.is_finite() || !(0.0..=1.0).contains(&score) {
        return Err(invalid_response(format!(
            "{label} must be finite and in 0.0..=1.0, got {score}"
        )));
    }
    Ok(())
}

fn invalid_response(message: impl Into<String>) -> VisionFfiError {
    VisionFfiError::fatal_with_code(
        VisionFfiErrorCode::InvalidResponse,
        "vision-provider-response",
        message,
    )
}

fn rect_contains(outer: VisionRect, inner: VisionRect) -> bool {
    let outer_right = i64::from(outer.x) + i64::from(outer.width);
    let outer_bottom = i64::from(outer.y) + i64::from(outer.height);
    let inner_right = i64::from(inner.x) + i64::from(inner.width);
    let inner_bottom = i64::from(inner.y) + i64::from(inner.height);
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.width > 0
        && inner.height > 0
        && inner_right <= outer_right
        && inner_bottom <= outer_bottom
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::slice;
    use std::sync::Arc;

    #[test]
    fn unattested_raw_ocr_result_fails_closed() {
        let frame = test_frame();
        let region = VisionRect::full_frame(&frame).expect("full frame rect");
        let request = OcrInferenceRequest {
            frame,
            region,
            languages: vec!["zh_cn".to_string()],
            timeout_ms: 1_000,
        };
        let mut boundary = VisionFfiBoundary::new(
            unsafe {
                FastDeployPpocrBackend::from_raw_functions(
                    fake_ocr_read_text_json,
                    fake_free_buffer,
                )
            },
            unsafe {
                OnnxRuntimeBackend::from_raw_functions(fake_nn_classify_json, fake_free_buffer)
            },
        );

        let err = boundary
            .read_text(request)
            .expect_err("unattested OCR result rejected");

        assert_eq!(err.code(), VisionFfiErrorCode::InvalidResponse);
        assert!(err.message().contains("without a session-bound"));
    }

    #[test]
    fn nn_classifies_frame() {
        let request = NnInferenceRequest {
            frame: test_frame(),
            model_id: "fixture-model-a".to_string(),
            labels: vec!["fixture.label".to_string(), "unknown".to_string()],
            timeout_ms: 1_000,
        };
        let mut boundary = VisionFfiBoundary::new(
            unsafe {
                FastDeployPpocrBackend::from_raw_functions(
                    fake_ocr_read_text_json,
                    fake_free_buffer,
                )
            },
            unsafe {
                OnnxRuntimeBackend::from_raw_functions(fake_nn_classify_json, fake_free_buffer)
            },
        );

        let result = boundary.classify(request).expect("nn result");

        assert_eq!(result.backend, VisionBackendKind::OnnxRuntime);
        assert_eq!(result.labels[0].label, "fixture.label");
        assert!(result.labels[0].score > 0.9);
    }

    #[test]
    fn invalid_frame_size_is_fatal() {
        let err = VisionFrame::new(2, 2, VisionPixelFormat::Rgb8, vec![0; 3])
            .expect_err("bad frame rejected");

        assert_eq!(err.severity(), VisionFfiErrorSeverity::Fatal);
        assert_eq!(err.module(), "vision-frame");
    }

    #[test]
    fn invalid_region_is_fatal() {
        let frame = test_frame();
        let request = OcrInferenceRequest {
            frame,
            region: VisionRect {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            },
            languages: vec!["zh_cn".to_string()],
            timeout_ms: 1_000,
        };

        let err = request.validate().expect_err("oversized region rejected");

        assert_eq!(err.severity(), VisionFfiErrorSeverity::Fatal);
        assert_eq!(err.module(), "vision-rect");
    }

    #[test]
    fn unavailable_ocr_backend_fails_loudly() {
        let frame = test_frame();
        let request = OcrInferenceRequest {
            region: VisionRect::full_frame(&frame).expect("full frame rect"),
            frame,
            languages: vec!["zh_cn".to_string()],
            timeout_ms: 1_000,
        };
        let mut backend = UnavailableOcrBackend;

        let err = backend.read_text(request).expect_err("unavailable backend");

        assert_eq!(err.severity(), VisionFfiErrorSeverity::Fatal);
        assert!(err.message().contains("not linked or configured"));
    }

    #[test]
    fn unavailable_nn_backend_fails_loudly() {
        let request = NnInferenceRequest {
            frame: test_frame(),
            model_id: "fixture-model-a".to_string(),
            labels: vec!["fixture.label".to_string()],
            timeout_ms: 1_000,
        };
        let mut backend = UnavailableNnBackend;

        let err = backend.classify(request).expect_err("unavailable backend");

        assert_eq!(err.severity(), VisionFfiErrorSeverity::Fatal);
        assert!(err.message().contains("not linked or configured"));
    }

    #[test]
    fn route_decision_requires_gpu_and_disables_directml() {
        let decision = r1_r3_route_decision();

        assert_eq!(
            decision.route,
            "ffi_boundary_then_fastdeploy_ppocr_and_onnxruntime"
        );
        assert_eq!(decision.ocr_backend, VisionBackendKind::FastDeployPpocr);
        assert_eq!(decision.nn_backend, VisionBackendKind::OnnxRuntime);
        assert!(decision.gpu_enabled);
        assert!(!decision.directml_enabled);
        assert_eq!(decision.expected_size_delta_mb, (150, 250));
    }

    #[test]
    fn ffi_nonzero_status_is_fatal() {
        let frame = test_frame();
        let request = OcrInferenceRequest {
            region: VisionRect::full_frame(&frame).expect("full frame rect"),
            frame,
            languages: vec!["zh_cn".to_string()],
            timeout_ms: 1_000,
        };
        let mut backend = unsafe {
            FastDeployPpocrBackend::from_raw_functions(fake_failing_json, fake_free_buffer)
        };

        let err = backend.read_text(request).expect_err("nonzero status");

        assert_eq!(err.severity(), VisionFfiErrorSeverity::Fatal);
        assert!(err.message().contains("status 7"));
    }

    #[test]
    fn ffi_timeout_status_is_typed() {
        let frame = test_frame();
        let request = OcrInferenceRequest {
            region: VisionRect::full_frame(&frame).expect("full frame rect"),
            frame,
            languages: vec!["zh_cn".to_string()],
            timeout_ms: 1_000,
        };
        let mut backend = unsafe {
            FastDeployPpocrBackend::from_raw_functions(fake_timeout_json, fake_free_buffer)
        };

        let err = backend.read_text(request).expect_err("timeout status");

        assert_eq!(err.code(), VisionFfiErrorCode::Timeout);
        assert!(err.message().contains("status 3"));
    }

    #[test]
    fn nan_confidence_is_typed_invalid_response() {
        let frame = test_frame();
        let request = OcrInferenceRequest {
            region: VisionRect::full_frame(&frame).expect("full frame rect"),
            frame,
            languages: vec!["en".to_string()],
            timeout_ms: 1_000,
        };
        let result = OcrInferenceResult {
            text: "invalid".to_string(),
            blocks: vec![OcrTextBlock {
                text: "invalid".to_string(),
                rect: request.region,
                confidence: Some(f32::NAN),
            }],
            confidence: Some(f32::NAN),
            backend: VisionBackendKind::FastDeployPpocr,
            warnings: Vec::new(),
        };

        let err = result
            .validate(&request)
            .expect_err("NaN confidence rejected");

        assert_eq!(err.code(), VisionFfiErrorCode::InvalidResponse);
    }

    #[test]
    fn missing_ffi_library_is_fatal() {
        let err = match FastDeployPpocrBackend::from_library_path("missing-fastdeploy-ppocr.dll") {
            Ok(_) => panic!("missing library was accepted"),
            Err(err) => err,
        };

        assert_eq!(err.severity(), VisionFfiErrorSeverity::Fatal);
        assert!(err.message().contains("failed to load"));
    }

    #[test]
    fn backend_from_manifest_requires_artifact_files() {
        let manifest = VisionProviderArtifactManifest {
            schema_version: VISION_PROVIDER_ARTIFACTS_SCHEMA_VERSION.to_string(),
            fastdeploy_ppocr: Some(test_ocr_artifacts()),
            onnxruntime: Some(test_nn_artifacts()),
        };

        let err = match FastDeployPpocrBackend::from_manifest(&manifest) {
            Ok(_) => panic!("missing OCR artifacts were accepted"),
            Err(err) => err,
        };

        assert_eq!(err.severity(), VisionFfiErrorSeverity::Fatal);
        assert!(err.message().contains("required artifact"));
    }

    #[test]
    fn backend_from_manifest_requires_backend_section() {
        let manifest = VisionProviderArtifactManifest {
            schema_version: VISION_PROVIDER_ARTIFACTS_SCHEMA_VERSION.to_string(),
            fastdeploy_ppocr: None,
            onnxruntime: None,
        };

        let err = match OnnxRuntimeBackend::from_manifest(&manifest) {
            Ok(_) => panic!("missing NN section was accepted"),
            Err(err) => err,
        };

        assert_eq!(err.severity(), VisionFfiErrorSeverity::Fatal);
        assert!(err.message().contains("onnxruntime"));
    }

    #[test]
    fn ocr_artifact_envelope_reads_text_from_frame() {
        let frame = test_frame();
        let request = OcrInferenceRequest {
            region: VisionRect::full_frame(&frame).expect("full frame rect"),
            frame,
            languages: vec!["zh_cn".to_string()],
            timeout_ms: 1_000,
        };
        let mut backend = unsafe {
            FastDeployPpocrBackend::from_raw_functions_with_artifacts_and_inventory(
                fake_ocr_envelope_json,
                fake_free_buffer,
                test_ocr_artifacts(),
                Some(test_cuda_inventory()),
            )
            .expect("test artifact backend")
        };

        let result = backend.read_text(request).expect("ocr result");

        assert_eq!(result.backend, VisionBackendKind::FastDeployPpocr);
        assert!(result.text.contains("artifact envelope"));
    }

    #[test]
    fn cpu_ocr_session_accepts_cpu_attestation_without_cuda_inventory() {
        let mut artifacts = test_ocr_artifacts();
        artifacts.execution_provider = Some(OnnxExecutionProvider::Cpu);
        artifacts.cuda_device = None;
        let mut backend = unsafe {
            FastDeployPpocrBackend::from_raw_functions_with_artifacts_and_inventory(
                fake_ocr_envelope_json,
                fake_free_buffer,
                artifacts,
                None,
            )
            .expect("CPU test backend")
        };

        let result = backend
            .read_text(test_ocr_request())
            .expect("CPU-attested OCR result");
        let session = backend.session_for_test().expect("CPU session");

        assert_eq!(result.backend, VisionBackendKind::FastDeployPpocr);
        assert_eq!(
            session.key().requested_backend(),
            OnnxExecutionProvider::Cpu
        );
        assert!(session.key().requested_cuda_device().is_none());
        assert!(session.key().resolved_cuda_device().is_none());
    }

    #[test]
    fn adapter_rejects_backend_device_identity_and_replay_mismatches() {
        let cases: [(&str, VisionFfiInvokeJson); 13] = [
            ("cpu-observed-cuda", fake_cpu_observed_cuda_json),
            ("cuda-observed-cpu", fake_cuda_observed_cpu_json),
            ("cuda-device-a-observed-b", fake_wrong_cuda_device_json),
            ("response-replay", fake_replayed_invocation_json),
            ("wrong-generation", fake_wrong_generation_json),
            ("wrong-model-ref", fake_wrong_model_ref_json),
            ("wrong-model", fake_wrong_model_json),
            ("wrong-configuration", fake_wrong_configuration_json),
            ("wrong-provider-binary", fake_wrong_provider_binary_json),
            ("wrong-runtime-version", fake_wrong_runtime_version_json),
            ("forged-fallback-observation", fake_fallback_observed_json),
            ("wrong-provider-registration", fake_wrong_registration_json),
            (
                "unknown-attestation-field",
                fake_unknown_attestation_field_json,
            ),
        ];

        for (name, invoke) in cases {
            let mut artifacts = test_ocr_artifacts();
            if name == "cpu-observed-cuda" {
                artifacts.execution_provider = Some(OnnxExecutionProvider::Cpu);
                artifacts.cuda_device = None;
            }
            let inventory = if artifacts.execution_provider == Some(OnnxExecutionProvider::Cuda) {
                Some(test_cuda_inventory())
            } else {
                None
            };
            let mut backend = unsafe {
                FastDeployPpocrBackend::from_raw_functions_with_artifacts_and_inventory(
                    invoke,
                    fake_free_buffer,
                    artifacts,
                    inventory,
                )
                .expect(name)
            };

            let err = backend.read_text(test_ocr_request()).expect_err(name);

            assert_eq!(err.code(), VisionFfiErrorCode::InvalidResponse, "{name}");
        }
    }

    #[test]
    fn adapter_rejects_missing_or_incomplete_attestation() {
        for (name, invoke) in [
            (
                "missing-attestation",
                fake_missing_attestation_json as VisionFfiInvokeJson,
            ),
            (
                "incomplete-attestation",
                fake_incomplete_attestation_json as VisionFfiInvokeJson,
            ),
        ] {
            let mut backend = unsafe {
                FastDeployPpocrBackend::from_raw_functions_with_artifacts_and_inventory(
                    invoke,
                    fake_free_buffer,
                    test_ocr_artifacts(),
                    Some(test_cuda_inventory()),
                )
                .expect(name)
            };

            let err = backend.read_text(test_ocr_request()).expect_err(name);

            assert_eq!(err.code(), VisionFfiErrorCode::InvalidResponse, "{name}");
        }
    }

    #[test]
    fn cuda_selection_is_deterministic_and_unavailable_inventory_fails_closed() {
        let backend = unsafe {
            FastDeployPpocrBackend::from_raw_functions_with_artifacts_and_inventory(
                fake_ocr_envelope_json,
                fake_free_buffer,
                test_ocr_artifacts(),
                Some(test_cuda_inventory()),
            )
            .expect("multi-GPU backend")
        };
        let selected = backend
            .session_for_test()
            .expect("session")
            .key()
            .resolved_cuda_device()
            .cloned()
            .expect("selected CUDA device");

        assert_eq!(selected.ordinal, 1);
        assert_eq!(
            selected.stable_identity,
            "cuda-uuid:11111111111111111111111111111111"
        );

        let err = match unsafe {
            FastDeployPpocrBackend::from_raw_functions_with_artifacts_and_inventory(
                fake_ocr_envelope_json,
                fake_free_buffer,
                test_ocr_artifacts(),
                None,
            )
        } {
            Ok(_) => panic!("unavailable CUDA inventory was accepted"),
            Err(err) => err,
        };
        assert_eq!(err.code(), VisionFfiErrorCode::ProviderUnavailable);
    }

    #[test]
    fn reconfiguration_keeps_in_flight_generation_immutable() {
        let mut backend = unsafe {
            FastDeployPpocrBackend::from_raw_functions_with_artifacts_and_inventory(
                fake_ocr_envelope_json,
                fake_free_buffer,
                test_ocr_artifacts(),
                Some(test_cuda_inventory()),
            )
            .expect("CUDA backend")
        };
        let in_flight = backend.session_for_test().expect("old generation");
        let err = backend
            .reconfigure_with_inventory_for_test(test_ocr_artifacts(), None)
            .expect_err("unavailable replacement must not commit");
        assert_eq!(err.code(), VisionFfiErrorCode::ProviderUnavailable);
        assert!(Arc::ptr_eq(
            &in_flight,
            &backend
                .session_for_test()
                .expect("unchanged generation after failed replacement")
        ));
        let mut cpu_artifacts = test_ocr_artifacts();
        cpu_artifacts.execution_provider = Some(OnnxExecutionProvider::Cpu);
        cpu_artifacts.cuda_device = None;

        backend
            .reconfigure_with_inventory_for_test(cpu_artifacts, None)
            .expect("atomic CPU reconfiguration");
        let current = backend.session_for_test().expect("new generation");

        assert_eq!(in_flight.generation(), 1);
        assert_eq!(
            in_flight.key().requested_backend(),
            OnnxExecutionProvider::Cuda
        );
        assert_eq!(current.generation(), 2);
        assert_eq!(
            current.key().requested_backend(),
            OnnxExecutionProvider::Cpu
        );
        assert_eq!(in_flight.session_id(), current.session_id());
        assert!(!Arc::ptr_eq(&in_flight, &current));
        backend
            .read_text(test_ocr_request())
            .expect("new CPU generation result");
    }

    #[test]
    fn nn_artifact_envelope_classifies_frame() {
        let request = NnInferenceRequest {
            frame: test_frame(),
            model_id: "page-classifier".to_string(),
            labels: vec!["home".to_string(), "unknown".to_string()],
            timeout_ms: 1_000,
        };
        let mut backend = unsafe {
            OnnxRuntimeBackend::from_raw_functions_with_artifacts(
                fake_nn_envelope_json,
                fake_free_buffer,
                test_nn_artifacts(),
            )
            .expect("test artifact backend")
        };

        let result = backend.classify(request).expect("nn result");

        assert_eq!(result.backend, VisionBackendKind::OnnxRuntime);
        assert_eq!(result.labels[0].label, "home");
    }

    #[test]
    fn vision_frame_serializes_pixels_as_base64_not_number_array() {
        let frame =
            VisionFrame::new(2, 2, VisionPixelFormat::Rgb8, (0_u8..12).collect()).expect("frame");

        let json = serde_json::to_string(&frame).expect("serialize");
        let decoded: VisionFrame = serde_json::from_str(&json).expect("deserialize");

        assert!(json.contains(r#""pixels":"AAECAwQFBgcICQoL""#));
        assert_eq!(decoded, frame);
    }

    #[test]
    fn vision_frame_rejects_invalid_base64_pixel_payloads() {
        let cases = [
            (
                "length-not-multiple-of-four",
                r#"{"width":1,"height":1,"pixel_format":"gray8","pixels":"AAA"}"#,
                "multiple of 4",
            ),
            (
                "invalid-byte",
                r#"{"width":1,"height":1,"pixel_format":"gray8","pixels":"AA?="}"#,
                "invalid byte",
            ),
            (
                "invalid-padding",
                r#"{"width":1,"height":1,"pixel_format":"gray8","pixels":"AA=A"}"#,
                "invalid padding",
            ),
        ];

        for (name, json, expected) in cases {
            let err: serde_json::Error = serde_json::from_str::<VisionFrame>(json).expect_err(name);
            assert!(err.to_string().contains(expected), "{name} produced {err}");
        }
    }

    #[test]
    fn vision_frame_round_trips_base64_padding_payload() {
        let frame = VisionFrame::new(1, 1, VisionPixelFormat::Gray8, vec![42]).expect("frame");

        let json = serde_json::to_string(&frame).expect("serialize");
        let decoded: VisionFrame = serde_json::from_str(&json).expect("deserialize");

        assert!(json.contains(r#""pixels":"Kg==""#));
        assert_eq!(decoded, frame);
    }

    #[test]
    fn base64_pixel_payload_stays_near_raw_frame_size() {
        let pixels = vec![7_u8; 1920 * 1080 * 3];

        let encoded_len = base64_pixels::encoded_len(&pixels);

        assert!(encoded_len <= pixels.len() * 3 / 2);
    }

    fn test_frame() -> VisionFrame {
        VisionFrame::new(2, 2, VisionPixelFormat::Rgb8, vec![0; 12]).expect("test frame")
    }

    fn test_ocr_request() -> OcrInferenceRequest {
        let frame = test_frame();
        OcrInferenceRequest {
            region: VisionRect::full_frame(&frame).expect("full frame rect"),
            frame,
            languages: vec!["zh_cn".to_string()],
            timeout_ms: 1_000,
        }
    }

    unsafe extern "C" fn fake_ocr_read_text_json(
        request_ptr: *const u8,
        request_len: usize,
        response_out: *mut VisionFfiOwnedBuffer,
    ) -> i32 {
        let request = read_ffi_request::<OcrInferenceRequest>(request_ptr, request_len);
        write_ffi_response(
            response_out,
            &OcrInferenceResult {
                text: "公开招募 09:00".to_string(),
                blocks: vec![OcrTextBlock {
                    text: "公开招募".to_string(),
                    rect: request.region,
                    confidence: Some(0.98),
                }],
                confidence: Some(0.98),
                backend: VisionBackendKind::FastDeployPpocr,
                warnings: Vec::new(),
            },
        )
    }

    unsafe extern "C" fn fake_nn_classify_json(
        request_ptr: *const u8,
        request_len: usize,
        response_out: *mut VisionFfiOwnedBuffer,
    ) -> i32 {
        let request = read_ffi_request::<NnInferenceRequest>(request_ptr, request_len);
        write_ffi_response(
            response_out,
            &NnClassificationResult {
                labels: vec![NnLabel {
                    label: request.labels[0].clone(),
                    score: 0.97,
                }],
                backend: VisionBackendKind::OnnxRuntime,
            },
        )
    }

    unsafe extern "C" fn fake_failing_json(
        _request_ptr: *const u8,
        _request_len: usize,
        response_out: *mut VisionFfiOwnedBuffer,
    ) -> i32 {
        write_ffi_response(response_out, "fake backend failure");
        7
    }

    unsafe extern "C" fn fake_timeout_json(
        _request_ptr: *const u8,
        _request_len: usize,
        response_out: *mut VisionFfiOwnedBuffer,
    ) -> i32 {
        write_ffi_response(response_out, "fake backend timeout");
        3
    }

    unsafe extern "C" fn fake_ocr_envelope_json(
        request_ptr: *const u8,
        request_len: usize,
        response_out: *mut VisionFfiOwnedBuffer,
    ) -> i32 {
        fake_attested_ocr_json(
            request_ptr,
            request_len,
            response_out,
            AttestationTamper::None,
        )
    }

    #[derive(Clone, Copy)]
    enum AttestationTamper {
        None,
        CpuObservedCuda,
        CudaObservedCpu,
        WrongCudaDevice,
        ReplayedInvocation,
        WrongGeneration,
        WrongModelRef,
        WrongModel,
        WrongConfiguration,
        WrongProviderBinary,
        WrongRuntimeVersion,
        FallbackObserved,
        WrongRegistration,
        UnknownAttestationField,
        Missing,
        Incomplete,
    }

    unsafe extern "C" fn fake_cpu_observed_cuda_json(
        request_ptr: *const u8,
        request_len: usize,
        response_out: *mut VisionFfiOwnedBuffer,
    ) -> i32 {
        fake_attested_ocr_json(
            request_ptr,
            request_len,
            response_out,
            AttestationTamper::CpuObservedCuda,
        )
    }

    unsafe extern "C" fn fake_cuda_observed_cpu_json(
        request_ptr: *const u8,
        request_len: usize,
        response_out: *mut VisionFfiOwnedBuffer,
    ) -> i32 {
        fake_attested_ocr_json(
            request_ptr,
            request_len,
            response_out,
            AttestationTamper::CudaObservedCpu,
        )
    }

    unsafe extern "C" fn fake_wrong_cuda_device_json(
        request_ptr: *const u8,
        request_len: usize,
        response_out: *mut VisionFfiOwnedBuffer,
    ) -> i32 {
        fake_attested_ocr_json(
            request_ptr,
            request_len,
            response_out,
            AttestationTamper::WrongCudaDevice,
        )
    }

    unsafe extern "C" fn fake_replayed_invocation_json(
        request_ptr: *const u8,
        request_len: usize,
        response_out: *mut VisionFfiOwnedBuffer,
    ) -> i32 {
        fake_attested_ocr_json(
            request_ptr,
            request_len,
            response_out,
            AttestationTamper::ReplayedInvocation,
        )
    }

    unsafe extern "C" fn fake_wrong_generation_json(
        request_ptr: *const u8,
        request_len: usize,
        response_out: *mut VisionFfiOwnedBuffer,
    ) -> i32 {
        fake_attested_ocr_json(
            request_ptr,
            request_len,
            response_out,
            AttestationTamper::WrongGeneration,
        )
    }

    unsafe extern "C" fn fake_wrong_model_json(
        request_ptr: *const u8,
        request_len: usize,
        response_out: *mut VisionFfiOwnedBuffer,
    ) -> i32 {
        fake_attested_ocr_json(
            request_ptr,
            request_len,
            response_out,
            AttestationTamper::WrongModel,
        )
    }

    unsafe extern "C" fn fake_wrong_model_ref_json(
        request_ptr: *const u8,
        request_len: usize,
        response_out: *mut VisionFfiOwnedBuffer,
    ) -> i32 {
        fake_attested_ocr_json(
            request_ptr,
            request_len,
            response_out,
            AttestationTamper::WrongModelRef,
        )
    }

    unsafe extern "C" fn fake_wrong_configuration_json(
        request_ptr: *const u8,
        request_len: usize,
        response_out: *mut VisionFfiOwnedBuffer,
    ) -> i32 {
        fake_attested_ocr_json(
            request_ptr,
            request_len,
            response_out,
            AttestationTamper::WrongConfiguration,
        )
    }

    unsafe extern "C" fn fake_wrong_provider_binary_json(
        request_ptr: *const u8,
        request_len: usize,
        response_out: *mut VisionFfiOwnedBuffer,
    ) -> i32 {
        fake_attested_ocr_json(
            request_ptr,
            request_len,
            response_out,
            AttestationTamper::WrongProviderBinary,
        )
    }

    unsafe extern "C" fn fake_wrong_runtime_version_json(
        request_ptr: *const u8,
        request_len: usize,
        response_out: *mut VisionFfiOwnedBuffer,
    ) -> i32 {
        fake_attested_ocr_json(
            request_ptr,
            request_len,
            response_out,
            AttestationTamper::WrongRuntimeVersion,
        )
    }

    unsafe extern "C" fn fake_fallback_observed_json(
        request_ptr: *const u8,
        request_len: usize,
        response_out: *mut VisionFfiOwnedBuffer,
    ) -> i32 {
        fake_attested_ocr_json(
            request_ptr,
            request_len,
            response_out,
            AttestationTamper::FallbackObserved,
        )
    }

    unsafe extern "C" fn fake_wrong_registration_json(
        request_ptr: *const u8,
        request_len: usize,
        response_out: *mut VisionFfiOwnedBuffer,
    ) -> i32 {
        fake_attested_ocr_json(
            request_ptr,
            request_len,
            response_out,
            AttestationTamper::WrongRegistration,
        )
    }

    unsafe extern "C" fn fake_unknown_attestation_field_json(
        request_ptr: *const u8,
        request_len: usize,
        response_out: *mut VisionFfiOwnedBuffer,
    ) -> i32 {
        fake_attested_ocr_json(
            request_ptr,
            request_len,
            response_out,
            AttestationTamper::UnknownAttestationField,
        )
    }

    unsafe extern "C" fn fake_missing_attestation_json(
        request_ptr: *const u8,
        request_len: usize,
        response_out: *mut VisionFfiOwnedBuffer,
    ) -> i32 {
        fake_attested_ocr_json(
            request_ptr,
            request_len,
            response_out,
            AttestationTamper::Missing,
        )
    }

    unsafe extern "C" fn fake_incomplete_attestation_json(
        request_ptr: *const u8,
        request_len: usize,
        response_out: *mut VisionFfiOwnedBuffer,
    ) -> i32 {
        fake_attested_ocr_json(
            request_ptr,
            request_len,
            response_out,
            AttestationTamper::Incomplete,
        )
    }

    fn fake_attested_ocr_json(
        request_ptr: *const u8,
        request_len: usize,
        response_out: *mut VisionFfiOwnedBuffer,
        tamper: AttestationTamper,
    ) -> i32 {
        let envelope = read_ffi_request::<FastDeployPpocrInvokeRequest>(request_ptr, request_len);
        let result = OcrInferenceResult {
            text: format!(
                "artifact envelope: {}",
                envelope.artifacts.provider_library_path.display()
            ),
            blocks: vec![OcrTextBlock {
                text: envelope.artifacts.supported_languages[0].clone(),
                rect: envelope.request.region,
                confidence: Some(0.95),
            }],
            confidence: Some(0.95),
            backend: VisionBackendKind::FastDeployPpocr,
            warnings: Vec::new(),
        };
        let mut response =
            serde_json::to_value(attested_ocr_response(&envelope, result)).expect("response JSON");
        match tamper {
            AttestationTamper::None => {}
            AttestationTamper::CpuObservedCuda => {
                response["attestation"]["resolved_execution_provider"] = serde_json::json!("cuda");
            }
            AttestationTamper::CudaObservedCpu => {
                response["attestation"]["resolved_execution_provider"] = serde_json::json!("cpu");
            }
            AttestationTamper::WrongCudaDevice => {
                response["attestation"]["session"]["key"]["resolved_cuda_device"]["stable_identity"] =
                    serde_json::json!("cuda-uuid:22222222222222222222222222222222");
            }
            AttestationTamper::ReplayedInvocation => {
                let replay = serde_json::json!("ocr-invocation-ffffffffffffffff");
                response["invocation_id"] = replay.clone();
                response["attestation"]["invocation_id"] = replay;
            }
            AttestationTamper::WrongGeneration => {
                response["session_generation"] = serde_json::json!(999);
                response["attestation"]["session"]["generation"] = serde_json::json!(999);
            }
            AttestationTamper::WrongModelRef => {
                response["attestation"]["session"]["key"]["model_ref"] =
                    serde_json::json!("wrong-model");
            }
            AttestationTamper::WrongModel => {
                response["attestation"]["session"]["key"]["model_sha256"] =
                    serde_json::json!("f".repeat(64));
            }
            AttestationTamper::WrongConfiguration => {
                response["attestation"]["session"]["key"]["provider_options_sha256"] =
                    serde_json::json!("f".repeat(64));
            }
            AttestationTamper::WrongProviderBinary => {
                response["attestation"]["provider"]["binary_sha256"] =
                    serde_json::json!("f".repeat(64));
            }
            AttestationTamper::WrongRuntimeVersion => {
                response["attestation"]["runtime"]["onnxruntime_version"] =
                    serde_json::json!("0.0.0-wrong");
            }
            AttestationTamper::FallbackObserved => {
                response["attestation"]["fallback_observed"] = serde_json::json!(false);
            }
            AttestationTamper::WrongRegistration => {
                response["attestation"]["registered_execution_providers"] =
                    serde_json::json!(["cpu", "cuda"]);
            }
            AttestationTamper::UnknownAttestationField => {
                response["attestation"]["unapproved_fact"] = serde_json::json!(true);
            }
            AttestationTamper::Missing => {
                response
                    .as_object_mut()
                    .expect("response object")
                    .remove("attestation");
            }
            AttestationTamper::Incomplete => {
                response["attestation"]["complete"] = serde_json::json!(false);
            }
        }
        write_ffi_response(response_out, &response)
    }

    unsafe extern "C" fn fake_nn_envelope_json(
        request_ptr: *const u8,
        request_len: usize,
        response_out: *mut VisionFfiOwnedBuffer,
    ) -> i32 {
        let envelope = read_ffi_request::<OnnxRuntimeInvokeRequest>(request_ptr, request_len);
        write_ffi_response(
            response_out,
            &NnClassificationResult {
                labels: vec![NnLabel {
                    label: envelope.artifacts.labels[0].clone(),
                    score: 0.96,
                }],
                backend: VisionBackendKind::OnnxRuntime,
            },
        )
    }

    unsafe extern "C" fn fake_free_buffer(buffer: VisionFfiOwnedBuffer) {
        if !buffer.data.is_null() {
            // SAFETY: test fake backends allocate every returned buffer from a Vec<u8>
            // and transfer its original length/capacity through VisionFfiOwnedBuffer.
            unsafe {
                drop(Vec::from_raw_parts(
                    buffer.data,
                    buffer.len,
                    buffer.capacity,
                ));
            }
        }
    }

    fn read_ffi_request<T>(request_ptr: *const u8, request_len: usize) -> T
    where
        T: for<'de> Deserialize<'de>,
    {
        // SAFETY: test callers pass a non-null request pointer and exact length
        // produced by the production FFI adapter serialization path.
        let bytes = unsafe { slice::from_raw_parts(request_ptr, request_len) };
        serde_json::from_slice(bytes).expect("decode fake FFI request")
    }

    fn write_ffi_response<T>(response_out: *mut VisionFfiOwnedBuffer, response: &T) -> i32
    where
        T: Serialize + ?Sized,
    {
        let mut bytes = serde_json::to_vec(response).expect("encode fake FFI response");
        let buffer = VisionFfiOwnedBuffer {
            data: bytes.as_mut_ptr(),
            len: bytes.len(),
            capacity: bytes.capacity(),
        };
        std::mem::forget(bytes);
        // SAFETY: test callers pass a valid output pointer owned by the FFI adapter.
        unsafe {
            response_out.write(buffer);
        }
        0
    }

    fn test_ocr_artifacts() -> FastDeployPpocrArtifacts {
        let detector_hash = "a".repeat(64);
        let recognizer_hash = "b".repeat(64);
        let dictionary_hash = "c".repeat(64);
        let model_hash =
            ppocr_model_content_sha256(&detector_hash, &recognizer_hash, &dictionary_hash, None)
                .expect("fixture model hash");
        FastDeployPpocrArtifacts {
            provider_library_path: PathBuf::from(
                "external-tools/vision/fastdeploy/ac_fastdeploy_ppocr.dll",
            ),
            provider_library_sha256: Some("e".repeat(64)),
            runtime_library_paths: vec![PathBuf::from(
                "external-tools/vision/fastdeploy/onnxruntime.dll",
            )],
            runtime_library_path: Some(PathBuf::from(
                "external-tools/vision/fastdeploy/onnxruntime.dll",
            )),
            runtime_library_sha256: Some("d".repeat(64)),
            detector_model_path: PathBuf::from("external-tools/vision/ppocr/det/inference.pdmodel"),
            recognizer_model_path: PathBuf::from(
                "external-tools/vision/ppocr/rec/inference.pdmodel",
            ),
            dictionary_path: PathBuf::from("external-tools/vision/ppocr/ppocr_keys_v1.txt"),
            classifier_model_path: None,
            model_ref: Some(PPOCR_V6_MEDIUM_MODEL_REF.to_string()),
            model_sha256: Some(model_hash),
            detector_model_sha256: Some(detector_hash),
            recognizer_model_sha256: Some(recognizer_hash),
            dictionary_sha256: Some(dictionary_hash),
            classifier_model_sha256: None,
            execution_provider: Some(OnnxExecutionProvider::Cuda),
            cuda_device: Some(test_cuda_selector()),
            strict_no_fallback: Some(true),
            supported_languages: vec!["zh_cn".to_string(), "en".to_string()],
            default_timeout_ms: 1_000,
        }
    }

    fn test_cuda_selector() -> CudaDeviceSelector {
        CudaDeviceSelector {
            ordinal: 1,
            expected_stable_identity: "cuda-uuid:11111111111111111111111111111111".to_string(),
        }
    }

    fn test_cuda_inventory() -> CudaDeviceInventory {
        CudaDeviceInventory {
            driver_version: 12_800,
            devices: vec![
                CudaDeviceIdentity {
                    ordinal: 0,
                    stable_identity: "cuda-uuid:00000000000000000000000000000000".to_string(),
                    pci_bus_id: Some("0000:01:00.0".to_string()),
                },
                CudaDeviceIdentity {
                    ordinal: 1,
                    stable_identity: "cuda-uuid:11111111111111111111111111111111".to_string(),
                    pci_bus_id: Some("0000:02:00.0".to_string()),
                },
            ],
        }
    }

    fn attested_ocr_response(
        envelope: &FastDeployPpocrInvokeRequest,
        result: OcrInferenceResult,
    ) -> FastDeployPpocrInvokeResponse {
        let cuda_driver_version = match envelope.session.key().requested_backend() {
            OnnxExecutionProvider::Cpu => None,
            OnnxExecutionProvider::Cuda => Some(12_800),
        };
        FastDeployPpocrInvokeResponse {
            schema_version: OCR_PROVIDER_RESPONSE_SCHEMA_VERSION.to_string(),
            invocation_id: envelope.invocation_id.clone(),
            session_id: envelope.session.session_id().clone(),
            session_generation: envelope.session.generation(),
            result,
            attestation: OcrExecutionAttestation {
                schema_version: OCR_EXECUTION_ATTESTATION_SCHEMA_VERSION.to_string(),
                invocation_id: envelope.invocation_id.clone(),
                session: envelope.session.clone(),
                resolved_execution_provider: envelope.session.key().requested_backend(),
                provider: OcrProviderBuildIdentity {
                    implementation: "actingcommand-ppocr-onnx-json".to_string(),
                    crate_version: "0.1.0-test".to_string(),
                    build_git_sha: None,
                    binary_sha256: envelope.session.key().provider_library_sha256().to_string(),
                },
                runtime: OcrRuntimeBuildIdentity {
                    onnxruntime_version: envelope.session.key().onnxruntime_version().to_string(),
                    onnxruntime_build_info: "ORT Build Info: fake test runtime".to_string(),
                    cuda_driver_version,
                    cuda_runtime_version: None,
                    cudnn_version: None,
                },
                registered_execution_providers: vec![envelope.session.key().requested_backend()],
                cpu_ep_registered: envelope.session.key().requested_backend()
                    == OnnxExecutionProvider::Cpu,
                cpu_fallback_disabled: envelope.session.key().requested_backend()
                    == OnnxExecutionProvider::Cuda,
                fallback_policy: OcrFallbackPolicy::Forbidden,
                fallback_observed: None,
                complete: true,
            },
        }
    }

    fn test_nn_artifacts() -> OnnxRuntimeArtifacts {
        OnnxRuntimeArtifacts {
            provider_library_path: PathBuf::from(
                "external-tools/vision/onnxruntime/ac_onnxruntime.dll",
            ),
            runtime_library_path: Some(PathBuf::from(
                "external-tools/vision/onnxruntime/onnxruntime.dll",
            )),
            model_path: PathBuf::from(
                "external-tools/vision/onnxruntime/models/page_classifier.onnx",
            ),
            model_ref: Some("page-classifier".to_string()),
            model_sha256: Some("d".repeat(64)),
            labels: vec!["home".to_string(), "unknown".to_string()],
            labels_path: None,
            execution_provider: OnnxExecutionProvider::Cpu,
            default_timeout_ms: 1_000,
        }
    }
}
