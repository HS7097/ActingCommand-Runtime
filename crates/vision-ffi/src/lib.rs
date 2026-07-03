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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisionFfiErrorSeverity {
    Fatal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisionFfiError {
    severity: VisionFfiErrorSeverity,
    module: &'static str,
    message: String,
}

impl VisionFfiError {
    pub fn fatal(module: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity: VisionFfiErrorSeverity::Fatal,
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
            return Err(VisionFfiError::fatal(
                "ocr",
                "OCR request must include at least one language",
            ));
        }
        if self.timeout_ms == 0 {
            return Err(VisionFfiError::fatal(
                "ocr",
                "OCR request timeout_ms must be non-zero",
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
            return Err(VisionFfiError::fatal(
                "nn",
                "NN request model_id must be non-empty",
            ));
        }
        if self.labels.is_empty() {
            return Err(VisionFfiError::fatal(
                "nn",
                "NN request must include at least one candidate label",
            ));
        }
        if self.timeout_ms == 0 {
            return Err(VisionFfiError::fatal(
                "nn",
                "NN request timeout_ms must be non-zero",
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
        gpu_enabled: false,
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
        Err(VisionFfiError::fatal(
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
        Err(VisionFfiError::fatal(
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::slice;

    #[test]
    fn ocr_reads_text_from_frame() {
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

        let result = boundary.read_text(request).expect("ocr result");

        assert_eq!(result.backend, VisionBackendKind::FastDeployPpocr);
        assert!(result.text.contains("公开招募"));
        assert_eq!(result.blocks.len(), 1);
    }

    #[test]
    fn nn_classifies_frame() {
        let request = NnInferenceRequest {
            frame: test_frame(),
            model_id: "ak-recruit-entry".to_string(),
            labels: vec!["arknights.recruit".to_string(), "unknown".to_string()],
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
        assert_eq!(result.labels[0].label, "arknights.recruit");
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
            model_id: "ak-recruit-entry".to_string(),
            labels: vec!["arknights.recruit".to_string()],
            timeout_ms: 1_000,
        };
        let mut backend = UnavailableNnBackend;

        let err = backend.classify(request).expect_err("unavailable backend");

        assert_eq!(err.severity(), VisionFfiErrorSeverity::Fatal);
        assert!(err.message().contains("not linked or configured"));
    }

    #[test]
    fn route_decision_disables_gpu_and_directml() {
        let decision = r1_r3_route_decision();

        assert_eq!(
            decision.route,
            "ffi_boundary_then_fastdeploy_ppocr_and_onnxruntime"
        );
        assert_eq!(decision.ocr_backend, VisionBackendKind::FastDeployPpocr);
        assert_eq!(decision.nn_backend, VisionBackendKind::OnnxRuntime);
        assert!(!decision.gpu_enabled);
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
            FastDeployPpocrBackend::from_raw_functions_with_artifacts(
                fake_ocr_envelope_json,
                fake_free_buffer,
                test_ocr_artifacts(),
            )
            .expect("test artifact backend")
        };

        let result = backend.read_text(request).expect("ocr result");

        assert_eq!(result.backend, VisionBackendKind::FastDeployPpocr);
        assert!(result.text.contains("artifact envelope"));
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

    unsafe extern "C" fn fake_ocr_envelope_json(
        request_ptr: *const u8,
        request_len: usize,
        response_out: *mut VisionFfiOwnedBuffer,
    ) -> i32 {
        let envelope = read_ffi_request::<FastDeployPpocrInvokeRequest>(request_ptr, request_len);
        write_ffi_response(
            response_out,
            &OcrInferenceResult {
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
            },
        )
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
        FastDeployPpocrArtifacts {
            provider_library_path: PathBuf::from(
                "external-tools/vision/fastdeploy/ac_fastdeploy_ppocr.dll",
            ),
            runtime_library_paths: vec![PathBuf::from(
                "external-tools/vision/fastdeploy/fastdeploy_ppocr_maa.dll",
            )],
            detector_model_path: PathBuf::from("external-tools/vision/ppocr/det/inference.pdmodel"),
            recognizer_model_path: PathBuf::from(
                "external-tools/vision/ppocr/rec/inference.pdmodel",
            ),
            dictionary_path: PathBuf::from("external-tools/vision/ppocr/ppocr_keys_v1.txt"),
            classifier_model_path: None,
            supported_languages: vec!["zh_cn".to_string(), "en".to_string()],
            default_timeout_ms: 1_000,
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
            labels: vec!["home".to_string(), "unknown".to_string()],
            labels_path: None,
            execution_provider: OnnxExecutionProvider::Cpu,
            default_timeout_ms: 1_000,
        }
    }
}
