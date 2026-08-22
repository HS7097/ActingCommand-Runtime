// SPDX-License-Identifier: AGPL-3.0-only

use crate::{ExecutionKernelError, ExecutionKernelResult};
use actingcommand_contract::{ApplicationLifecycleAction, InstanceId, MonitorObservation};
use actingcommand_device::{CaptureBackend, DeviceResult, Frame, InputBackend};
pub use actingcommand_recognition_pack::VisionProvider as RecognitionVisionProvider;
use actingcommand_recognition_pack::{
    NnProviderLabel, NnProviderRequest, NnProviderResult, OcrExecutionProviderKind,
    OcrProviderExecutionEvidence, OcrProviderObservation, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, PackRect, VisionProviderError, VisionProviderErrorCode,
    VisionProviderFrame,
};
use actingcommand_vision_ffi::{
    NnClassificationResult, NnEngine, NnInferenceRequest, OcrEngine, OcrExecutionAttestation,
    OcrFallbackPolicy, OcrInferenceOutput, OcrInferenceRequest, OcrInferenceResult,
    OnnxExecutionProvider, VisionBackendKind, VisionFfiError, VisionFfiErrorCode, VisionFrame,
    VisionPixelFormat, VisionRect,
};
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

const MAX_MODEL_REF_BYTES: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisionModelIdentity {
    model_ref: String,
    model_sha256: String,
}

impl VisionModelIdentity {
    pub fn new(
        model_ref: impl Into<String>,
        model_sha256: impl Into<String>,
    ) -> Result<Self, VisionProviderError> {
        let model_ref = model_ref.into();
        let model_sha256 = model_sha256.into();
        if model_ref.trim().is_empty()
            || model_ref.len() > MAX_MODEL_REF_BYTES
            || model_ref.contains(['/', '\\', ':'])
            || model_ref.chars().any(char::is_control)
        {
            return Err(VisionProviderError::new(
                VisionProviderErrorCode::ModelMismatch,
                "vision model_ref must be a bounded logical identifier, not a host path",
            ));
        }
        if model_sha256.len() != 64
            || !model_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(VisionProviderError::new(
                VisionProviderErrorCode::ModelMismatch,
                "vision model_sha256 must be exactly 64 lowercase hexadecimal characters",
            ));
        }
        Ok(Self {
            model_ref,
            model_sha256,
        })
    }

    pub fn model_ref(&self) -> &str {
        &self.model_ref
    }

    pub fn model_sha256(&self) -> &str {
        &self.model_sha256
    }
}

struct OcrCapability {
    identity: VisionModelIdentity,
    engine: Mutex<Option<Box<dyn OcrEngine + Send>>>,
}

struct NnCapability {
    identity: VisionModelIdentity,
    engine: Mutex<Option<Box<dyn NnEngine + Send>>>,
}

/// Thread-safe Runtime adapter over the existing mutable vision-ffi engines.
pub struct VisionFfiProvider {
    ocr: Option<OcrCapability>,
    nn: Option<NnCapability>,
}

impl VisionFfiProvider {
    pub fn new(
        ocr: Option<(Box<dyn OcrEngine + Send>, VisionModelIdentity)>,
        nn: Option<(Box<dyn NnEngine + Send>, VisionModelIdentity)>,
    ) -> Result<Self, VisionProviderError> {
        if ocr.is_none() && nn.is_none() {
            return Err(VisionProviderError::new(
                VisionProviderErrorCode::Unavailable,
                "vision provider must expose at least one production capability",
            ));
        }
        Ok(Self {
            ocr: ocr.map(|(engine, identity)| OcrCapability {
                identity,
                engine: Mutex::new(Some(engine)),
            }),
            nn: nn.map(|(engine, identity)| NnCapability {
                identity,
                engine: Mutex::new(Some(engine)),
            }),
        })
    }
}

impl fmt::Debug for VisionFfiProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VisionFfiProvider")
            .field(
                "ocr_model_ref",
                &self
                    .ocr
                    .as_ref()
                    .map(|capability| capability.identity.model_ref()),
            )
            .field(
                "nn_model_ref",
                &self
                    .nn
                    .as_ref()
                    .map(|capability| capability.identity.model_ref()),
            )
            .finish()
    }
}

impl RecognitionVisionProvider for VisionFfiProvider {
    fn require_ocr_model(
        &self,
        model_ref: &str,
        model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        require_model(
            self.ocr.as_ref().map(|capability| &capability.identity),
            model_ref,
            model_sha256,
            "OCR",
        )
    }

    fn require_nn_model(
        &self,
        model_ref: &str,
        model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        require_model(
            self.nn.as_ref().map(|capability| &capability.identity),
            model_ref,
            model_sha256,
            "NN",
        )
    }

    fn read_text(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        self.require_ocr_model(request.model_ref, request.model_sha256)?;
        let capability = self.ocr.as_ref().ok_or_else(|| unavailable("OCR"))?;
        let frame = copy_frame(request.frame)?;
        let region = VisionRect {
            x: request.region.x,
            y: request.region.y,
            width: request.region.width,
            height: request.region.height,
        };
        let ffi_request = OcrInferenceRequest {
            frame,
            region,
            languages: request.languages.to_vec(),
            timeout_ms: request.timeout_ms,
        };
        let result = invoke_ocr(capability, ffi_request)?;
        validate_ocr_backend(&result)?;
        Ok(map_ocr_result(result))
    }

    fn read_text_with_execution_evidence(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderObservation, VisionProviderError> {
        self.require_ocr_model(request.model_ref, request.model_sha256)?;
        let capability = self.ocr.as_ref().ok_or_else(|| unavailable("OCR"))?;
        let frame = copy_frame(request.frame)?;
        let ffi_request = OcrInferenceRequest {
            frame,
            region: VisionRect {
                x: request.region.x,
                y: request.region.y,
                width: request.region.width,
                height: request.region.height,
            },
            languages: request.languages.to_vec(),
            timeout_ms: request.timeout_ms,
        };
        let output = invoke_ocr_with_attestation(capability, ffi_request)?;
        validate_ocr_backend(&output.result)?;
        let attestation = output.execution_attestation.ok_or_else(|| {
            VisionProviderError::new(
                VisionProviderErrorCode::InvalidResponse,
                "OCR engine did not return execution attestation",
            )
        })?;
        let execution = map_ocr_execution_evidence(&attestation, &capability.identity)?;
        Ok(OcrProviderObservation {
            result: map_ocr_result(output.result),
            execution: Some(execution),
        })
    }

    fn classify(
        &self,
        request: NnProviderRequest<'_>,
    ) -> Result<NnProviderResult, VisionProviderError> {
        self.require_nn_model(request.model_ref, request.model_sha256)?;
        let capability = self.nn.as_ref().ok_or_else(|| unavailable("NN"))?;
        let frame = crop_frame(request.frame, request.region)?;
        let ffi_request = NnInferenceRequest {
            frame,
            model_id: request.model_ref.to_string(),
            labels: request.candidate_labels.to_vec(),
            timeout_ms: request.timeout_ms,
        };
        let result = invoke_nn(capability, ffi_request)?;
        validate_nn_backend(&result)?;
        Ok(NnProviderResult {
            labels: result
                .labels
                .into_iter()
                .map(|label| NnProviderLabel {
                    label: label.label,
                    score: label.score,
                })
                .collect(),
        })
    }
}

fn require_model(
    available: Option<&VisionModelIdentity>,
    model_ref: &str,
    model_sha256: &str,
    capability: &str,
) -> Result<(), VisionProviderError> {
    let available = available.ok_or_else(|| unavailable(capability))?;
    if available.model_ref() != model_ref || available.model_sha256() != model_sha256 {
        return Err(VisionProviderError::new(
            VisionProviderErrorCode::ModelMismatch,
            format!(
                "{capability} model identity does not match the admitted production capability"
            ),
        ));
    }
    Ok(())
}

fn invoke_ocr(
    capability: &OcrCapability,
    request: OcrInferenceRequest,
) -> Result<OcrInferenceResult, VisionProviderError> {
    let mut slot = capability.engine.lock().map_err(|_| {
        VisionProviderError::new(
            VisionProviderErrorCode::Internal,
            "OCR engine mutex is poisoned",
        )
    })?;
    let engine = slot.as_mut().ok_or_else(|| {
        VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "OCR engine was retired after a provider panic",
        )
    })?;
    match catch_unwind(AssertUnwindSafe(|| engine.read_text(request))) {
        Ok(result) => result.map_err(map_ffi_error),
        Err(_) => {
            *slot = None;
            Err(VisionProviderError::new(
                VisionProviderErrorCode::Internal,
                "OCR engine panicked and was retired",
            ))
        }
    }
}

fn invoke_ocr_with_attestation(
    capability: &OcrCapability,
    request: OcrInferenceRequest,
) -> Result<OcrInferenceOutput, VisionProviderError> {
    let mut slot = capability.engine.lock().map_err(|_| {
        VisionProviderError::new(
            VisionProviderErrorCode::Internal,
            "OCR engine mutex is poisoned",
        )
    })?;
    let engine = slot.as_mut().ok_or_else(|| {
        VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "OCR engine was retired after a provider panic",
        )
    })?;
    match catch_unwind(AssertUnwindSafe(|| {
        engine.read_text_with_attestation(request)
    })) {
        Ok(result) => result.map_err(map_ffi_error),
        Err(_) => {
            *slot = None;
            Err(VisionProviderError::new(
                VisionProviderErrorCode::Internal,
                "OCR engine panicked and was retired",
            ))
        }
    }
}

fn map_ocr_result(result: OcrInferenceResult) -> OcrProviderResult {
    OcrProviderResult {
        text: result.text,
        blocks: result
            .blocks
            .into_iter()
            .map(|block| OcrProviderTextBlock {
                text: block.text,
                rect: PackRect {
                    x: block.rect.x,
                    y: block.rect.y,
                    width: block.rect.width,
                    height: block.rect.height,
                },
                confidence: block.confidence,
            })
            .collect(),
        confidence: result.confidence,
    }
}

fn map_ocr_execution_evidence(
    attestation: &OcrExecutionAttestation,
    identity: &VisionModelIdentity,
) -> Result<OcrProviderExecutionEvidence, VisionProviderError> {
    let key = attestation.session.key();
    if key.model_ref() != identity.model_ref() || key.model_sha256() != identity.model_sha256() {
        return Err(VisionProviderError::new(
            VisionProviderErrorCode::ModelMismatch,
            "OCR execution attestation model identity does not match the admitted capability",
        ));
    }
    let provider_kind = |provider| match provider {
        OnnxExecutionProvider::Cpu => OcrExecutionProviderKind::Cpu,
        OnnxExecutionProvider::Cuda => OcrExecutionProviderKind::Cuda,
    };
    Ok(OcrProviderExecutionEvidence {
        invocation_id: attestation.invocation_id.as_str().to_string(),
        session_id: attestation.session.session_id().as_str().to_string(),
        session_generation: attestation.session.generation(),
        requested_provider: provider_kind(key.requested_backend()),
        resolved_provider: provider_kind(attestation.resolved_execution_provider),
        requested_cuda_ordinal: key.requested_cuda_device().map(|device| device.ordinal),
        requested_cuda_identity: key
            .requested_cuda_device()
            .map(|device| device.expected_stable_identity.clone()),
        resolved_cuda_ordinal: key.resolved_cuda_device().map(|device| device.ordinal),
        resolved_cuda_identity: key
            .resolved_cuda_device()
            .map(|device| device.stable_identity.clone()),
        provider_implementation: attestation.provider.implementation.clone(),
        provider_binary_sha256: attestation.provider.binary_sha256.clone(),
        runtime_version: attestation.runtime.onnxruntime_version.clone(),
        model_ref: key.model_ref().to_string(),
        model_sha256: key.model_sha256().to_string(),
        cpu_ep_registered: attestation.cpu_ep_registered,
        cpu_fallback_disabled: attestation.cpu_fallback_disabled,
        fallback_forbidden: attestation.fallback_policy == OcrFallbackPolicy::Forbidden,
        fallback_observed: attestation.fallback_observed,
        complete: attestation.complete,
    })
}

fn invoke_nn(
    capability: &NnCapability,
    request: NnInferenceRequest,
) -> Result<NnClassificationResult, VisionProviderError> {
    let mut slot = capability.engine.lock().map_err(|_| {
        VisionProviderError::new(
            VisionProviderErrorCode::Internal,
            "NN engine mutex is poisoned",
        )
    })?;
    let engine = slot.as_mut().ok_or_else(|| {
        VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "NN engine was retired after a provider panic",
        )
    })?;
    match catch_unwind(AssertUnwindSafe(|| engine.classify(request))) {
        Ok(result) => result.map_err(map_ffi_error),
        Err(_) => {
            *slot = None;
            Err(VisionProviderError::new(
                VisionProviderErrorCode::Internal,
                "NN engine panicked and was retired",
            ))
        }
    }
}

fn copy_frame(frame: VisionProviderFrame<'_>) -> Result<VisionFrame, VisionProviderError> {
    VisionFrame::new(
        frame.width,
        frame.height,
        VisionPixelFormat::Rgb8,
        frame.rgb8_pixels.to_vec(),
    )
    .map_err(map_ffi_error)
}

fn crop_frame(
    frame: VisionProviderFrame<'_>,
    region: PackRect,
) -> Result<VisionFrame, VisionProviderError> {
    let x = usize::try_from(region.x).map_err(|_| invalid_region())?;
    let y = usize::try_from(region.y).map_err(|_| invalid_region())?;
    let width = usize::try_from(region.width).map_err(|_| invalid_region())?;
    let height = usize::try_from(region.height).map_err(|_| invalid_region())?;
    let frame_width = usize::try_from(frame.width).map_err(|_| invalid_region())?;
    let frame_height = usize::try_from(frame.height).map_err(|_| invalid_region())?;
    let end_x = x.checked_add(width).ok_or_else(invalid_region)?;
    let end_y = y.checked_add(height).ok_or_else(invalid_region)?;
    if width == 0 || height == 0 || end_x > frame_width || end_y > frame_height {
        return Err(invalid_region());
    }
    let row_bytes = width.checked_mul(3).ok_or_else(invalid_region)?;
    let capacity = row_bytes.checked_mul(height).ok_or_else(invalid_region)?;
    let frame_row_bytes = frame_width.checked_mul(3).ok_or_else(invalid_region)?;
    let mut pixels = Vec::with_capacity(capacity);
    for row in y..end_y {
        let start = row
            .checked_mul(frame_row_bytes)
            .and_then(|offset| x.checked_mul(3).and_then(|x| offset.checked_add(x)))
            .ok_or_else(invalid_region)?;
        let end = start.checked_add(row_bytes).ok_or_else(invalid_region)?;
        let source = frame
            .rgb8_pixels
            .get(start..end)
            .ok_or_else(invalid_region)?;
        pixels.extend_from_slice(source);
    }
    VisionFrame::new(
        u32::try_from(width).map_err(|_| invalid_region())?,
        u32::try_from(height).map_err(|_| invalid_region())?,
        VisionPixelFormat::Rgb8,
        pixels,
    )
    .map_err(map_ffi_error)
}

fn validate_ocr_backend(result: &OcrInferenceResult) -> Result<(), VisionProviderError> {
    if result.backend != VisionBackendKind::FastDeployPpocr {
        return Err(VisionProviderError::new(
            VisionProviderErrorCode::InvalidResponse,
            "production OCR result did not attest the fastdeploy_ppocr backend",
        ));
    }
    if !result.warnings.is_empty() {
        return Err(VisionProviderError::new(
            VisionProviderErrorCode::InvalidResponse,
            "production OCR result reported unhandled provider warnings",
        ));
    }
    Ok(())
}

fn validate_nn_backend(result: &NnClassificationResult) -> Result<(), VisionProviderError> {
    if result.backend != VisionBackendKind::OnnxRuntime {
        return Err(VisionProviderError::new(
            VisionProviderErrorCode::InvalidResponse,
            "production NN result did not attest the onnxruntime backend",
        ));
    }
    Ok(())
}

fn map_ffi_error(error: VisionFfiError) -> VisionProviderError {
    let code = match error.code() {
        VisionFfiErrorCode::ProviderUnavailable => VisionProviderErrorCode::Unavailable,
        VisionFfiErrorCode::Timeout => VisionProviderErrorCode::Timeout,
        VisionFfiErrorCode::ModelMismatch => VisionProviderErrorCode::ModelMismatch,
        VisionFfiErrorCode::InvalidRequest | VisionFfiErrorCode::InvalidResponse => {
            VisionProviderErrorCode::InvalidResponse
        }
        VisionFfiErrorCode::ProviderFailure
        | VisionFfiErrorCode::ProviderPanic
        | VisionFfiErrorCode::Internal => VisionProviderErrorCode::Internal,
    };
    VisionProviderError::new(code, error.to_string())
}

fn unavailable(capability: &str) -> VisionProviderError {
    VisionProviderError::new(
        VisionProviderErrorCode::Unavailable,
        format!("{capability} production capability is unavailable"),
    )
}

fn invalid_region() -> VisionProviderError {
    VisionProviderError::new(
        VisionProviderErrorCode::InvalidResponse,
        "vision ROI is outside the RGB8 frame",
    )
}

/// Runtime-owned provenance for an execution backend instance.
///
/// Fixture simulation is an explicit zero-device boundary. It must not be accepted by normal
/// device-facing Runtime operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionBackendProvenance {
    PhysicalDevice,
    FixtureSimulation,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedExecutionInstance {
    instance_id: InstanceId,
    audit_endpoint: String,
    provenance: ExecutionBackendProvenance,
}

impl ResolvedExecutionInstance {
    pub fn new(instance_id: InstanceId, audit_endpoint: impl Into<String>) -> Self {
        Self {
            instance_id,
            audit_endpoint: audit_endpoint.into(),
            provenance: ExecutionBackendProvenance::PhysicalDevice,
        }
    }

    pub fn fixture_simulation(instance_id: InstanceId) -> Self {
        Self {
            instance_id,
            audit_endpoint: "fixture-simulation".to_owned(),
            provenance: ExecutionBackendProvenance::FixtureSimulation,
        }
    }

    pub const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    pub fn audit_endpoint(&self) -> &str {
        &self.audit_endpoint
    }

    pub const fn provenance(&self) -> ExecutionBackendProvenance {
        self.provenance
    }
}

impl fmt::Debug for ResolvedExecutionInstance {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedExecutionInstance")
            .field("instance_id", &self.instance_id)
            .field("audit_endpoint", &"<redacted>")
            .field("provenance", &self.provenance)
            .finish()
    }
}

/// Daemon-only factory boundary. Implementations open backends inside execution worker threads.
///
/// Backend implementations have no outcome-fact ingress. Scheduling outcomes must be committed by
/// Runtime from the terminal ledger fact instead of being supplied by a backend:
///
/// ```compile_fail
/// use actingcommand_execution_kernel::ExecutionBackendProvider;
///
/// fn inject_outcome(provider: &dyn ExecutionBackendProvider) {
///     let _ = provider.outcomes();
/// }
/// ```
pub trait ExecutionBackendProvider: Send + Sync + 'static {
    fn instance_aliases(&self) -> Vec<String>;

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance>;

    fn open_input(&self, instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>>;

    fn open_capture(&self, instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>>;

    fn control_application(
        &self,
        instance_alias: &str,
        action: ApplicationLifecycleAction,
    ) -> DeviceResult<()>;

    fn vision_provider(&self) -> Option<Arc<dyn RecognitionVisionProvider>> {
        None
    }

    fn observe_monitor(
        &self,
        _instance_alias: &str,
        _expected_page: &str,
        _frame: &Frame,
    ) -> ExecutionKernelResult<MonitorObservation> {
        Err(ExecutionKernelError::fatal(
            "monitor_observation_unavailable",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_vision_ffi::{NnLabel, OcrTextBlock};
    use serde_json::json;

    #[test]
    fn vision_model_identity_rejects_host_paths_and_noncanonical_hashes() {
        for (model_ref, hash) in [
            ("C:\\models\\model.onnx", "a".repeat(64)),
            ("logical-model", "A".repeat(64)),
            ("logical-model", "a".repeat(63)),
        ] {
            let error =
                VisionModelIdentity::new(model_ref, hash).expect_err("invalid model identity");
            assert_eq!(error.code(), VisionProviderErrorCode::ModelMismatch);
        }
    }

    #[test]
    fn nn_adapter_crops_roi_before_calling_existing_ffi_engine() {
        let observed = Arc::new(Mutex::new(None));
        let provider = VisionFfiProvider::new(
            None,
            Some((
                Box::new(RecordingNnEngine {
                    observed: Arc::clone(&observed),
                    backend: VisionBackendKind::OnnxRuntime,
                }),
                identity("fixture-model", 'b'),
            )),
        )
        .expect("provider");
        let pixels = [
            1, 2, 3, 4, 5, 6, //
            7, 8, 9, 10, 11, 12,
        ];
        let labels = vec!["home".to_string()];
        let model_sha256 = "b".repeat(64);

        let result = RecognitionVisionProvider::classify(
            &provider,
            NnProviderRequest {
                frame: VisionProviderFrame {
                    width: 2,
                    height: 2,
                    rgb8_pixels: &pixels,
                },
                region: PackRect {
                    x: 1,
                    y: 0,
                    width: 1,
                    height: 2,
                },
                model_ref: "fixture-model",
                model_sha256: &model_sha256,
                candidate_labels: &labels,
                timeout_ms: 1_000,
            },
        )
        .expect("classification");

        assert_eq!(result.labels[0].label, "home");
        let request = observed
            .lock()
            .expect("observed request lock")
            .clone()
            .expect("request observed");
        assert_eq!((request.frame.width, request.frame.height), (1, 2));
        assert_eq!(request.frame.pixels, vec![4, 5, 6, 10, 11, 12]);
    }

    #[test]
    fn adapter_accepts_canonical_non_full_frame_ocr_roi() {
        let provider = VisionFfiProvider::new(
            Some((
                Box::new(StaticOcrEngine {
                    backend: VisionBackendKind::FastDeployPpocr,
                    warnings: Vec::new(),
                }),
                identity("PP-OCRv6_medium", 'a'),
            )),
            None,
        )
        .expect("provider");
        let pixels = [0; 12];
        let languages = vec!["en".to_string()];
        let model_sha256 = "a".repeat(64);
        let region = PackRect {
            x: 1,
            y: 0,
            width: 1,
            height: 2,
        };

        let result = RecognitionVisionProvider::read_text(
            &provider,
            OcrProviderRequest {
                frame: VisionProviderFrame {
                    width: 2,
                    height: 2,
                    rgb8_pixels: &pixels,
                },
                region,
                languages: &languages,
                timeout_ms: 1_000,
                model_ref: "PP-OCRv6_medium",
                model_sha256: &model_sha256,
            },
        )
        .expect("canonical ROI accepted");

        assert_eq!(result.text, "home");
        assert_eq!(result.blocks.len(), 1);
        assert_eq!(result.blocks[0].rect, region);
        assert_eq!(result.blocks[0].confidence, Some(0.99));
    }

    #[test]
    fn adapter_maps_cpu_and_cuda_execution_attestation_and_rejects_model_mismatch() {
        for (provider_name, cuda_ordinal) in [("cpu", None), ("cuda", Some(3))] {
            let provider = VisionFfiProvider::new(
                Some((
                    Box::new(AttestedOcrEngine {
                        attestation: execution_attestation(provider_name, cuda_ordinal, 'a'),
                    }),
                    identity("PP-OCRv6_medium", 'a'),
                )),
                None,
            )
            .expect("provider");
            let result = RecognitionVisionProvider::read_text_with_execution_evidence(
                &provider,
                ocr_request(),
            )
            .expect("attested OCR");
            let evidence = result.execution.expect("execution evidence");
            assert_eq!(evidence.model_ref, "PP-OCRv6_medium");
            assert_eq!(evidence.model_sha256, "a".repeat(64));
            assert_eq!(evidence.requested_provider, evidence.resolved_provider);
            assert_eq!(evidence.requested_cuda_ordinal, cuda_ordinal);
            assert_eq!(evidence.resolved_cuda_ordinal, cuda_ordinal);
            assert_eq!(evidence.cpu_ep_registered, provider_name == "cpu");
            assert_eq!(evidence.cpu_fallback_disabled, provider_name == "cuda");
            assert!(evidence.fallback_forbidden);
            assert_eq!(evidence.fallback_observed, None);
        }

        let provider = VisionFfiProvider::new(
            Some((
                Box::new(AttestedOcrEngine {
                    attestation: execution_attestation("cpu", None, 'b'),
                }),
                identity("PP-OCRv6_medium", 'a'),
            )),
            None,
        )
        .expect("provider");
        let error =
            RecognitionVisionProvider::read_text_with_execution_evidence(&provider, ocr_request())
                .expect_err("attested model mismatch must fail closed");
        assert_eq!(error.code(), VisionProviderErrorCode::ModelMismatch);
    }

    #[test]
    fn adapter_rejects_any_ocr_provider_warning() {
        let provider = VisionFfiProvider::new(
            Some((
                Box::new(StaticOcrEngine {
                    backend: VisionBackendKind::FastDeployPpocr,
                    warnings: vec!["unexpected provider degradation".to_string()],
                }),
                identity("PP-OCRv6_medium", 'a'),
            )),
            None,
        )
        .expect("provider");
        let pixels = [0; 12];
        let languages = vec!["en".to_string()];
        let model_sha256 = "a".repeat(64);

        let error = RecognitionVisionProvider::read_text(
            &provider,
            OcrProviderRequest {
                frame: VisionProviderFrame {
                    width: 2,
                    height: 2,
                    rgb8_pixels: &pixels,
                },
                region: PackRect {
                    x: 1,
                    y: 0,
                    width: 1,
                    height: 2,
                },
                languages: &languages,
                timeout_ms: 1_000,
                model_ref: "PP-OCRv6_medium",
                model_sha256: &model_sha256,
            },
        )
        .expect_err("provider warning remains fail-closed");

        assert_eq!(error.code(), VisionProviderErrorCode::InvalidResponse);
    }

    #[test]
    fn adapter_rejects_test_double_backend_and_retires_panicking_engine() {
        let pixels = [0, 0, 0];
        let languages = vec!["en".to_string()];
        let model_sha256 = "a".repeat(64);
        let request = || OcrProviderRequest {
            frame: VisionProviderFrame {
                width: 1,
                height: 1,
                rgb8_pixels: &pixels,
            },
            region: PackRect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            languages: &languages,
            timeout_ms: 1_000,
            model_ref: "PP-OCRv6_medium",
            model_sha256: &model_sha256,
        };
        let test_double = VisionFfiProvider::new(
            Some((
                Box::new(StaticOcrEngine {
                    backend: VisionBackendKind::TestDouble,
                    warnings: Vec::new(),
                }),
                identity("PP-OCRv6_medium", 'a'),
            )),
            None,
        )
        .expect("provider");
        let error = RecognitionVisionProvider::read_text(&test_double, request())
            .expect_err("test double backend rejected");
        assert_eq!(error.code(), VisionProviderErrorCode::InvalidResponse);

        let panicking = VisionFfiProvider::new(
            Some((
                Box::new(PanickingOcrEngine),
                identity("PP-OCRv6_medium", 'a'),
            )),
            None,
        )
        .expect("provider");
        let first = RecognitionVisionProvider::read_text(&panicking, request())
            .expect_err("panic converted to typed error");
        assert_eq!(first.code(), VisionProviderErrorCode::Internal);
        let second = RecognitionVisionProvider::read_text(&panicking, request())
            .expect_err("panicked engine stays retired");
        assert_eq!(second.code(), VisionProviderErrorCode::Unavailable);
    }

    #[test]
    fn adapter_preserves_typed_provider_timeout() {
        let provider = VisionFfiProvider::new(
            Some((Box::new(TimeoutOcrEngine), identity("PP-OCRv6_medium", 'a'))),
            None,
        )
        .expect("provider");
        let pixels = [0, 0, 0];
        let languages = vec!["en".to_string()];
        let model_sha256 = "a".repeat(64);

        let error = RecognitionVisionProvider::read_text(
            &provider,
            OcrProviderRequest {
                frame: VisionProviderFrame {
                    width: 1,
                    height: 1,
                    rgb8_pixels: &pixels,
                },
                region: PackRect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                languages: &languages,
                timeout_ms: 1,
                model_ref: "PP-OCRv6_medium",
                model_sha256: &model_sha256,
            },
        )
        .expect_err("timeout remains typed");

        assert_eq!(error.code(), VisionProviderErrorCode::Timeout);
    }

    fn identity(model_ref: &str, hash_byte: char) -> VisionModelIdentity {
        VisionModelIdentity::new(model_ref, hash_byte.to_string().repeat(64)).expect("identity")
    }

    fn ocr_request<'a>() -> OcrProviderRequest<'a> {
        static PIXELS: [u8; 3] = [0, 0, 0];
        static LANGUAGES: [String; 1] = [String::new()];
        OcrProviderRequest {
            frame: VisionProviderFrame {
                width: 1,
                height: 1,
                rgb8_pixels: &PIXELS,
            },
            region: PackRect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            languages: &LANGUAGES,
            timeout_ms: 1_000,
            model_ref: "PP-OCRv6_medium",
            model_sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        }
    }

    fn execution_attestation(
        provider: &str,
        cuda_ordinal: Option<u32>,
        model_hash_byte: char,
    ) -> OcrExecutionAttestation {
        let requested_cuda_device = cuda_ordinal.map(|ordinal| {
            json!({
                "ordinal": ordinal,
                "expected_stable_identity": format!("cuda-{ordinal}")
            })
        });
        let resolved_cuda_device = cuda_ordinal.map(|ordinal| {
            json!({
                "ordinal": ordinal,
                "stable_identity": format!("cuda-{ordinal}"),
                "pci_bus_id": null
            })
        });
        serde_json::from_value(json!({
            "schema_version": "actingcommand.ocr_execution_attestation.v1",
            "invocation_id": "ocr-invocation-0000000000000001",
            "session": {
                "session_id": "ocr-session-0000000000000001",
                "generation": 1,
                "key": {
                    "provider_library_sha256": "b".repeat(64),
                    "runtime_library_path": "runtime.dll",
                    "runtime_library_sha256": "c".repeat(64),
                    "onnxruntime_version": "fixture-runtime",
                    "model_ref": "PP-OCRv6_medium",
                    "model_sha256": model_hash_byte.to_string().repeat(64),
                    "requested_backend": provider,
                    "requested_cuda_device": requested_cuda_device,
                    "resolved_cuda_device": resolved_cuda_device,
                    "provider_options_sha256": "d".repeat(64)
                }
            },
            "resolved_execution_provider": provider,
            "provider": {
                "implementation": "actingcommand-ppocr-onnx-json",
                "crate_version": "0.1.0",
                "build_git_sha": null,
                "binary_sha256": "b".repeat(64)
            },
            "runtime": {
                "onnxruntime_version": "fixture-runtime",
                "onnxruntime_build_info": "fixture",
                "cuda_driver_version": cuda_ordinal.map(|_| 1),
                "cuda_runtime_version": cuda_ordinal.map(|_| "fixture"),
                "cudnn_version": cuda_ordinal.map(|_| "fixture")
            },
            "registered_execution_providers": [provider],
            "cpu_ep_registered": provider == "cpu",
            "cpu_fallback_disabled": provider == "cuda",
            "fallback_policy": "forbidden",
            "fallback_observed": null,
            "complete": true
        }))
        .expect("fixture attestation")
    }

    struct RecordingNnEngine {
        observed: Arc<Mutex<Option<NnInferenceRequest>>>,
        backend: VisionBackendKind,
    }

    impl NnEngine for RecordingNnEngine {
        fn classify(
            &mut self,
            request: NnInferenceRequest,
        ) -> actingcommand_vision_ffi::VisionFfiResult<NnClassificationResult> {
            *self.observed.lock().expect("observed request lock") = Some(request);
            Ok(NnClassificationResult {
                labels: vec![NnLabel {
                    label: "home".to_string(),
                    score: 0.99,
                }],
                backend: self.backend,
            })
        }
    }

    struct StaticOcrEngine {
        backend: VisionBackendKind,
        warnings: Vec<String>,
    }

    struct AttestedOcrEngine {
        attestation: OcrExecutionAttestation,
    }

    impl OcrEngine for AttestedOcrEngine {
        fn read_text(
            &mut self,
            request: OcrInferenceRequest,
        ) -> actingcommand_vision_ffi::VisionFfiResult<OcrInferenceResult> {
            Ok(attested_result(request))
        }

        fn read_text_with_attestation(
            &mut self,
            request: OcrInferenceRequest,
        ) -> actingcommand_vision_ffi::VisionFfiResult<OcrInferenceOutput> {
            Ok(OcrInferenceOutput {
                result: attested_result(request),
                execution_attestation: Some(self.attestation.clone()),
            })
        }
    }

    fn attested_result(request: OcrInferenceRequest) -> OcrInferenceResult {
        OcrInferenceResult {
            text: "home".to_string(),
            blocks: vec![OcrTextBlock {
                text: "home".to_string(),
                rect: request.region,
                confidence: Some(0.99),
            }],
            confidence: Some(0.99),
            backend: VisionBackendKind::FastDeployPpocr,
            warnings: Vec::new(),
        }
    }

    impl OcrEngine for StaticOcrEngine {
        fn read_text(
            &mut self,
            request: OcrInferenceRequest,
        ) -> actingcommand_vision_ffi::VisionFfiResult<OcrInferenceResult> {
            Ok(OcrInferenceResult {
                text: "home".to_string(),
                blocks: vec![OcrTextBlock {
                    text: "home".to_string(),
                    rect: request.region,
                    confidence: Some(0.99),
                }],
                confidence: Some(0.99),
                backend: self.backend,
                warnings: self.warnings.clone(),
            })
        }
    }

    struct PanickingOcrEngine;

    impl OcrEngine for PanickingOcrEngine {
        fn read_text(
            &mut self,
            _request: OcrInferenceRequest,
        ) -> actingcommand_vision_ffi::VisionFfiResult<OcrInferenceResult> {
            panic!("fixture provider panic")
        }
    }

    struct TimeoutOcrEngine;

    impl OcrEngine for TimeoutOcrEngine {
        fn read_text(
            &mut self,
            _request: OcrInferenceRequest,
        ) -> actingcommand_vision_ffi::VisionFfiResult<OcrInferenceResult> {
            Err(VisionFfiError::fatal_with_code(
                VisionFfiErrorCode::Timeout,
                "fixture",
                "injected timeout",
            ))
        }
    }
}
