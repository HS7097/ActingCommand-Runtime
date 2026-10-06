// SPDX-License-Identifier: AGPL-3.0-only

//! Safe Rust boundary between the Runtime and its OCR and NN engines.
//!
//! The OCR and NN engines run in-process (Workflow #360): this crate defines the borrowed
//! request views, results and execution attestation they exchange with the Runtime, the model
//! folder rule, and the loader contract through which the Runtime builds one engine per model
//! on first use. Runtime callers cannot substitute mock recognition for production OCR or NN
//! results.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod artifacts;
pub mod ffi;
pub mod models;

pub use actingcommand_contract::{
    PPOCR_DIAGNOSTIC_RESULT_SCHEMA, PPOCR_MAX_BUSINESS_JSON_BYTES, PPOCR_MAX_DIAGNOSTIC_JSON_BYTES,
    PPOCR_MAX_DIAGNOSTIC_NODES, PPOCR_MAX_DIAGNOSTIC_REPORTS, PPOCR_MAX_ENVELOPE_BYTES,
    PPOCR_MAX_NODE_LOG_BYTES, PPOCR_MAX_REPORT_JSON_BYTES, PPOCR_MAX_RESPONSE_BYTES,
    PPOCR_NODE_PLACEMENT_RECORD_TYPE, PpocrCpuAssignedNodeDiagnostic, PpocrDiagnostics,
    PpocrNodePlacementDiagnostic, validate_ppocr_call_diagnostics,
};
pub use artifacts::*;
pub use ffi::*;
pub use models::*;
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
    #[serde(skip)]
    ppocr_diagnostics: PpocrDiagnostics,
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
            ppocr_diagnostics: Vec::new(),
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

    pub fn ppocr_diagnostics(&self) -> &PpocrDiagnostics {
        &self.ppocr_diagnostics
    }

    pub fn with_ppocr_diagnostics(mut self, diagnostics: PpocrDiagnostics) -> Self {
        self.ppocr_diagnostics.extend(diagnostics);
        self
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisionFrame {
    pub width: u32,
    pub height: u32,
    pub pixel_format: VisionPixelFormat,
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

    pub fn view(&self) -> VisionFrameView<'_> {
        VisionFrameView {
            width: self.width,
            height: self.height,
            pixel_format: self.pixel_format,
            pixels: &self.pixels,
        }
    }
}

/// Borrowed `VisionFrame`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisionFrameView<'a> {
    pub width: u32,
    pub height: u32,
    pub pixel_format: VisionPixelFormat,
    pub pixels: &'a [u8],
}

impl<'a> VisionFrameView<'a> {
    pub fn new(
        width: u32,
        height: u32,
        pixel_format: VisionPixelFormat,
        pixels: &'a [u8],
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrInferenceRequest {
    pub frame: VisionFrame,
    pub region: VisionRect,
    pub languages: Vec<String>,
    pub timeout_ms: u64,
}

impl OcrInferenceRequest {
    pub fn validate(&self) -> VisionFfiResult<()> {
        self.view().validate()
    }

    pub fn view(&self) -> OcrInferenceRequestView<'_> {
        OcrInferenceRequestView {
            frame: self.frame.view(),
            region: self.region,
            languages: &self.languages,
            timeout_ms: self.timeout_ms,
        }
    }
}

/// Borrowed `OcrInferenceRequest`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OcrInferenceRequestView<'a> {
    pub frame: VisionFrameView<'a>,
    pub region: VisionRect,
    pub languages: &'a [String],
    pub timeout_ms: u64,
}

impl OcrInferenceRequestView<'_> {
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

    /// The only whole-frame copy left on the OCR path: for engines that implement
    /// only the by-value `OcrEngine` methods.
    pub fn to_request(&self) -> VisionFfiResult<OcrInferenceRequest> {
        Ok(OcrInferenceRequest {
            frame: VisionFrame::new(
                self.frame.width,
                self.frame.height,
                self.frame.pixel_format,
                self.frame.pixels.to_vec(),
            )?,
            region: self.region,
            languages: self.languages.to_vec(),
            timeout_ms: self.timeout_ms,
        })
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
    #[serde(skip)]
    pub ppocr_diagnostics: PpocrDiagnostics,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OcrInferenceOutput {
    pub result: OcrInferenceResult,
    pub execution_attestation: Option<OcrExecutionAttestation>,
}

impl OcrInferenceResult {
    pub fn validate(&self, request: &OcrInferenceRequest) -> VisionFfiResult<()> {
        self.validate_for(request.view())
    }

    pub fn validate_for(&self, request: OcrInferenceRequestView<'_>) -> VisionFfiResult<()> {
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

    fn read_text_with_attestation(
        &mut self,
        request: OcrInferenceRequest,
    ) -> VisionFfiResult<OcrInferenceOutput> {
        self.read_text(request).map(|result| OcrInferenceOutput {
            result,
            execution_attestation: None,
        })
    }

    /// Borrowed request; the default copies once into the by-value method.
    fn read_text_view(
        &mut self,
        request: OcrInferenceRequestView<'_>,
    ) -> VisionFfiResult<OcrInferenceResult> {
        self.read_text(request.to_request()?)
    }

    /// Borrowed request; the default copies once into the by-value method.
    fn read_text_with_attestation_view(
        &mut self,
        request: OcrInferenceRequestView<'_>,
    ) -> VisionFfiResult<OcrInferenceOutput> {
        self.read_text_with_attestation(request.to_request()?)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
    fn nan_confidence_is_typed_invalid_response() {
        let frame = test_frame();
        let request = OcrInferenceRequest {
            region: VisionRect::full_frame(&frame).expect("full frame rect"),
            frame,
            languages: vec!["en".to_string()],
            timeout_ms: 1_000,
        };
        let result = OcrInferenceResult {
            ppocr_diagnostics: Vec::new(),
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

    fn test_frame() -> VisionFrame {
        VisionFrame::new(2, 2, VisionPixelFormat::Rgb8, vec![0; 12]).expect("test frame")
    }
}
